//! The document reading worker and tesseract (A03, plan D2): the only place
//! that starts them. Untrusted files are parsed only inside those two
//! processes, never here or in the desktop.
//!
//! [`DocreadHost::read`] reads a set of documents and returns one
//! [`JobOutcome`] each. It **blocks** (seconds per page): call it off the
//! egui thread. Per call it:
//!
//! * refuses empty and oversized files itself, splits the rest into
//!   batches within the protocol caps (jobs, bytes, templates);
//! * starts `clientledger-docread` with a scrubbed environment and an empty
//!   owner-only working folder, inside a kill-on-close job (Windows) or its
//!   own process group (Unix), and waits for `Ready` ([`DocreadLimits::ready`]);
//! * writes batches on their own thread and drains the worker's stdout and
//!   stderr concurrently; frames over their cap or garbled are a breach;
//! * enforces time: dispatch → first `PageStarted`, each page, and a job
//!   ceiling that grows by one page's time per page started, up to ten;
//! * watches memory every [`DocreadLimits::poll`]: commit on Windows
//!   (`PrivateUsage`; a working-set cap would let commit grow into the page
//!   file, review M2), resident memory elsewhere;
//! * runs tesseract for each `NeedOcr` page after checking its PGM header,
//!   with `TEMP`/`TMP`/`TMPDIR` at a fresh owner-only folder deleted after
//!   the page (Leptonica writes a temp file from stdin on MSVC), its own
//!   time and memory limits and an output cap;
//! * on a crash, timeout, memory breach or bad reply kills the worker and
//!   retries the job in a new one; a job that kills two workers is
//!   [`JobOutcome::CouldNotRead`] ("Couldn't read, enter by hand");
//! * recycles the worker after [`DocreadLimits::recycle_jobs`] jobs, or
//!   when its memory between jobs is over [`DocreadLimits::recycle_memory`];
//! * re-validates every `Reading` (sizes, boxes, spots, template index).
//!
//! [`DocreadHost::read_region`] (protocol version 4, A04 D4) re-reads one
//! box of one page through the same worker, limits and retries: the box
//! is cut out and OCR'd in the worker, never here. Its reply must be words
//! only, on that page, each centred in the box; anything else is a bad
//! reply (the worker is killed).
//!
//! Confinement (security review M1-M5):
//!
//! * Windows: the worker and each tesseract run in their own job:
//!   kill-on-close, a hard commit cap, one active process (they can start
//!   nothing) and no Windows Error Reporting (`procs/win_job_memory.rs`).
//!   If the job can't be set up the child is stopped at once and the
//!   document is "Couldn't read" (fail closed).
//! * Linux: the worker limits itself (`clientledger-docread` `limits.rs`:
//!   CPU, no core file, not dumpable, `SIGKILL` when the host dies); the
//!   host sets tesseract's CPU, data and core limits with `prlimit` before
//!   giving it its page.
//! * macOS: the worker's own CPU and core limits and the watchdog;
//!   tesseract has the watchdog only (P37).
//! * Everywhere: each running child is recorded under
//!   `<scratch>/.pids`; the next host start ([`sweep_docread_leftovers`])
//!   stops any that outlived a crashed host and removes its scratch
//!   folders.
//!
//! Logs carry outcome classes and counts only: never a file name, a path
//! inside a document, text or an amount. Worker stderr lines are logged
//! only when they are exactly one of the worker's fixed lines
//! ([`WORKER_LINES`]).

use crate::paths::create_private_dir;
use crate::procs::{self, TreeKiller};
use cl_core::CalendarDate;
use cl_docread::model::{MAX_STATEMENT_PAGES, PageWords, Reading, Spot, Template};
use cl_docread::pipeline::is_region_reading;
use cl_docread::protocol::{
    BatchOptions, Bytes, FrameError, FromWorker, Job, JobFailure, JobHint,
    MAX_BATCH_DOCUMENT_BYTES, MAX_DOCUMENT_BYTES, MAX_FROM_WORKER_FRAME, MAX_JOB_DONE_FRAME,
    MAX_JOBS_PER_BATCH, MAX_TEMPLATES, MAX_TO_WORKER_FRAME, MAX_TSV_BYTES, OcrFailure, OcrMode,
    PROTOCOL_VERSION, ToWorker, Tsv, region_ok, validate_pgm, write_frame,
};
use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

const MIB: u64 = 1024 * 1024;

/// Where tesseract and its model data are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrTools {
    /// The `tesseract` program (absolute).
    pub exe: PathBuf,
    /// The folder holding `eng.traineddata` (and `osd.traineddata`).
    pub tessdata: PathBuf,
}

impl OcrTools {
    /// The tools in an OCR folder (`tesseract[.exe]` and `tessdata/`).
    #[must_use]
    pub fn at(dir: &Path) -> Option<OcrTools> {
        crate::paths::is_ocr_dir(dir).then(|| OcrTools {
            exe: dir.join(crate::paths::tesseract_exe_name()),
            tessdata: dir.join("tessdata"),
        })
    }

    /// The installed (or, in debug builds, checkout) OCR tools, if any
    /// (`paths::find_ocr_dir`).
    #[must_use]
    pub fn find() -> Option<OcrTools> {
        crate::paths::find_ocr_dir().and_then(|d| OcrTools::at(&d))
    }
}

/// Time and memory limits (plan D2, Q-A03c), set from step 12a's
/// measurements (`cargo xtask docread-accuracy`, Windows commit, 2026-10-26):
/// the worker peaked at 78 MiB on 720 synthetic documents and at 759 MiB on
/// the largest picture it accepts (256 MiB decoded); tesseract at 56 MiB,
/// 110 MiB on a ten-page scanned PDF. An OCR page took 0.9 s at p95 (cold
/// start included), a ten-page scanned PDF 9 s. Linux and macOS resident
/// figures come from the `docread-accuracy` CI job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocreadLimits {
    /// Start → `Ready`.
    pub ready: Duration,
    /// Job dispatch → its first `PageStarted` (a load that hangs).
    pub first_page: Duration,
    /// Each page: from its `PageStarted`, or from the host's OCR answer.
    pub per_page: Duration,
    /// The job ceiling's fixed part: ceiling = this + pages started (at
    /// most ten, twenty for a statement) × `per_page`.
    pub job_base: Duration,
    /// One tesseract run.
    pub ocr_page: Duration,
    /// Worker memory watchdog (commit on Windows, resident elsewhere).
    pub worker_memory: u64,
    /// Tesseract memory watchdog.
    pub tesseract_memory: u64,
    /// Hard cap on Windows (job object), percent of the watchdog value.
    pub hard_cap_percent: u64,
    /// Watchdog poll interval.
    pub poll: Duration,
    /// Jobs one worker serves before it is replaced.
    pub recycle_jobs: u32,
    /// Worker memory between jobs above which it is replaced.
    pub recycle_memory: u64,
    /// Workers a job may kill before it is given up on.
    pub kills_per_job: u8,
    /// Attempts to start a worker before every waiting job is given up on.
    pub start_attempts: u8,
}

impl Default for DocreadLimits {
    fn default() -> Self {
        DocreadLimits {
            ready: Duration::from_secs(10),
            first_page: Duration::from_secs(30),
            per_page: Duration::from_secs(30),
            job_base: Duration::from_secs(30),
            ocr_page: Duration::from_secs(30),
            worker_memory: 1024 * MIB,
            tesseract_memory: 384 * MIB,
            hard_cap_percent: 125,
            poll: Duration::from_millis(200),
            recycle_jobs: 20,
            recycle_memory: 512 * MIB,
            kills_per_job: 2,
            start_attempts: 2,
        }
    }
}

impl DocreadLimits {
    /// The job ceiling once `pages` pages have started.
    #[must_use]
    pub fn job_ceiling(&self, pages: usize) -> Duration {
        let n = u32::try_from(pages.clamp(1, usize::from(MAX_STATEMENT_PAGES))).unwrap_or(1);
        self.job_base + self.per_page * n
    }

    fn hard_cap(&self, watchdog: u64) -> u64 {
        watchdog.saturating_mul(self.hard_cap_percent) / 100
    }
}

/// How to run the reader.
#[derive(Debug, Clone)]
pub struct DocreadConfig {
    /// The `clientledger-docread` program (`paths::find_docread_exe`).
    pub worker_exe: PathBuf,
    /// Tesseract, when installed; without it only text PDFs are read.
    pub ocr: Option<OcrTools>,
    /// Scratch root (`AppPaths::docread_dir`).
    pub scratch: PathBuf,
    pub limits: DocreadLimits,
    /// Extra worker environment for tests: only `CL_DOCREAD_TEST_*` names,
    /// and only honoured in builds with cl-host's `test-support` feature.
    pub test_env: Vec<(String, String)>,
}

impl DocreadConfig {
    /// The default configuration for an app data folder.
    #[must_use]
    pub fn new(paths: &crate::AppPaths, worker_exe: PathBuf, ocr: Option<OcrTools>) -> Self {
        DocreadConfig {
            worker_exe,
            ocr,
            scratch: paths.docread_dir(),
            limits: DocreadLimits::default(),
            test_env: Vec::new(),
        }
    }
}

/// One document to read. File names never cross to the worker. The bytes
/// are shared (`Arc`): the desktop's copy is handed over without copying;
/// the one copy made is the frame sent to the worker.
#[derive(Clone, PartialEq, Eq)]
pub struct DocJob {
    pub bytes: Arc<[u8]>,
    pub hint: JobHint,
    /// The company's layout templates (at most `MAX_TEMPLATES` are sent).
    pub templates: Vec<Template>,
}

impl std::fmt::Debug for DocJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocJob")
            .field("bytes", &self.bytes.len())
            .field("hint", &self.hint)
            .field("templates", &self.templates.len())
            .finish()
    }
}

/// One box of one page to re-read (A04 D4). The file name never crosses
/// to the worker; `Debug` shows only sizes.
#[derive(Clone, PartialEq, Eq)]
pub struct RegionJob {
    /// Shared with the desktop's copy (no copy until the frame is built).
    pub bytes: Arc<[u8]>,
    /// The page (from 0) and the box on the original page.
    pub region: Spot,
}

impl std::fmt::Debug for RegionJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegionJob")
            .field("bytes", &self.bytes.len())
            .field("page", &self.region.page)
            .finish_non_exhaustive()
    }
}

/// What became of a region re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionOutcome {
    /// The words centred in the box, on the original page (possibly none).
    Words(PageWords),
    /// The box is under 1 % of the page on a side, or the page is past the
    /// pages read (`protocol::region_ok`): nothing was started.
    Refused,
    /// Not readable for a plain reason (protected, needs OCR, ...).
    Failed(JobFailure),
    /// The reader gave up on it.
    CouldNotRead(CouldNotRead),
}

/// Why a document ended without a reading or a plain failure: the owner
/// sees "Couldn't read, enter by hand".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CouldNotRead {
    /// The worker crashed (twice) on this document.
    Crashed,
    /// The document took longer than its limits (twice).
    TimedOut,
    /// The worker used more memory than allowed (twice).
    TooMuchMemory,
    /// The worker sent something malformed (twice).
    BadReply,
    /// The worker could not be started.
    Unavailable,
    /// [`DocreadHost::cancel_handle`] was set.
    Cancelled,
}

/// What became of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// Read (the reading may still carry checks for the owner).
    Read(Box<Reading>),
    /// Not readable for a plain reason (protected, unsupported, ...).
    Failed(JobFailure),
    /// The reader gave up on it.
    CouldNotRead(CouldNotRead),
}

/// Counters for tests and the log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DocreadStats {
    pub workers_started: u32,
    pub kills: u32,
    pub recycles: u32,
    pub ocr_pages: u32,
    pub ocr_failures: u32,
    /// Pages read again with the best model (`OcrMode::ReadBest`).
    pub ocr_best_pages: u32,
    /// Single values read again (`OcrMode::Word`).
    pub ocr_rereads: u32,
    pub bad_images: u32,
    /// Highest worker memory seen (commit on Windows, resident elsewhere),
    /// in bytes: the process's own peak on Windows, the watchdog's samples
    /// elsewhere (step 12a measures limits with it).
    pub worker_peak_memory: u64,
    /// Highest tesseract memory seen, counted the same way.
    pub tesseract_peak_memory: u64,
}

/// Runs reading jobs (see the module comment).
#[derive(Debug)]
pub struct DocreadHost {
    config: DocreadConfig,
    stats: DocreadStats,
    cancel: Arc<AtomicBool>,
}

/// A distinct number for each scratch folder in this process.
static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

/// A fresh folder below `root`, removed on drop. Owner-only on Unix
/// (mode 0700); on Windows it inherits the app data folder's access list
/// (`%LOCALAPPDATA%`: the owner, SYSTEM and administrators; an explicit
/// list is P37).
#[derive(Debug)]
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(root: &Path, kind: &str) -> std::io::Result<ScratchDir> {
        let n = SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("{kind}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_private_dir(root).map_err(|e| std::io::Error::other(e.to_string()))?;
        create_private_dir(&dir).map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(ScratchDir(dir))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // A child that was just killed may hold a handle for a moment.
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        tracing::warn!("docread: a scratch folder could not be removed");
    }
}

// ---------------------------------------------------------------------------
// Leftovers of a host that crashed (security review M1, L10)
// ---------------------------------------------------------------------------

/// Where running children are recorded:
/// `<scratch>/.pids/<host pid>-<child pid>`.
fn records_dir(scratch: &Path) -> PathBuf {
    scratch.join(".pids")
}

/// A record of a running worker or tesseract (pid, start time, program),
/// removed on drop, so a later host can stop it if this one crashes.
#[derive(Debug)]
struct ChildRecord(PathBuf);

impl ChildRecord {
    fn new(scratch: &Path, exe: &Path, pid: u32) -> Option<ChildRecord> {
        let dir = records_dir(scratch);
        create_private_dir(&dir).ok()?;
        let (_, start) = procs::process_identity(pid)?;
        let path = dir.join(format!("{}-{pid}", std::process::id()));
        std::fs::write(&path, format!("{pid}\n{start}\n{}\n", exe.display())).ok()?;
        Some(ChildRecord(path))
    }
}

impl Drop for ChildRecord {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What [`sweep_docread_leftovers`] cleaned up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DocreadSwept {
    /// Workers or tesseract runs stopped.
    pub stopped: u32,
    /// Scratch folders removed.
    pub folders: u32,
}

/// Clean up after a host that crashed while reading: stop every recorded
/// worker or tesseract still running (same program and start time, so a
/// recycled pid is never touched) and remove the scratch folders other
/// processes left. Call once at host start, after the single-instance
/// lock is held (the supervisor does). Records and folders of this
/// process are kept.
#[must_use]
pub fn sweep_docread_leftovers(paths: &crate::AppPaths) -> DocreadSwept {
    sweep(&paths.docread_dir())
}

fn sweep(scratch: &Path) -> DocreadSwept {
    let me = std::process::id();
    let mut swept = DocreadSwept::default();
    if let Ok(entries) = std::fs::read_dir(records_dir(scratch)) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let host = name.split('-').next().and_then(|h| h.parse::<u32>().ok());
            if host == Some(me) {
                continue;
            }
            if reap_record(&e.path()) {
                swept.stopped += 1;
            }
            let _ = std::fs::remove_file(e.path());
        }
    }
    if let Ok(entries) = std::fs::read_dir(scratch) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_leftover_scratch(&name, me)
                && e.file_type().is_ok_and(|t| t.is_dir())
                && std::fs::remove_dir_all(e.path()).is_ok()
            {
                swept.folders += 1;
            }
        }
    }
    swept
}

/// A scratch folder name (`{w|t}-{pid}-{n}`) made by another process.
fn is_leftover_scratch(name: &str, me: u32) -> bool {
    let mut parts = name.split('-');
    matches!(parts.next(), Some("w" | "t"))
        && parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .is_some_and(|p| p != me)
        && parts.next().is_some_and(|n| n.parse::<u64>().is_ok())
        && parts.next().is_none()
}

/// Stop the process in one record when it is still the reader it names.
fn reap_record(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut lines = text.lines();
    let (Some(pid), Some(start), Some(exe)) = (
        lines.next().and_then(|l| l.parse::<u32>().ok()),
        lines.next().and_then(|l| l.parse::<u64>().ok()),
        lines.next().map(PathBuf::from),
    ) else {
        return false;
    };
    if pid == std::process::id() {
        return false;
    }
    // Only ever one of the two reading programs.
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name != crate::paths::docread_exe_name() && name != crate::paths::tesseract_exe_name() {
        return false;
    }
    let Some((live, started)) = procs::process_identity(pid) else {
        return false;
    };
    if started != start || !procs::same_file(&live, &exe) {
        return false;
    }
    procs::kill_process(pid)
}

/// The minimal environment for a child: system folders only, a private
/// temp and home.
fn scrubbed_env(tmp: &Path) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = Vec::new();
    for k in ["TEMP", "TMP", "TMPDIR", "HOME", "USERPROFILE"] {
        env.push((k.into(), tmp.as_os_str().to_owned()));
    }
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        let root_path = PathBuf::from(&root);
        let path = std::env::join_paths([root_path.join("System32"), root_path])
            .unwrap_or_else(|_| OsString::from(r"C:\Windows\System32"));
        env.push(("PATH".into(), path));
        env.push(("SystemRoot".into(), root.clone()));
        env.push(("windir".into(), root));
    } else {
        env.push(("PATH".into(), "/usr/bin:/bin".into()));
    }
    env
}

/// Memory of `pid` as the watchdog counts it: commit (`PrivateUsage`) on
/// Windows, resident elsewhere. `None` once the process is gone.
fn memory_of(sys: &mut System, pid: u32) -> Option<u64> {
    let pid = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_memory(),
    );
    let p = sys.process(pid)?;
    Some(if cfg!(windows) {
        p.virtual_memory()
    } else {
        p.memory()
    })
}

/// The process's own peak commit (`PeakPagefileUsage`), exact where the
/// watchdog's samples could miss a short spike. Windows only.
fn exact_peak(child: &Child) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        win32job::utils::get_process_memory_info(child.as_raw_handle() as isize)
            .ok()
            .and_then(|c| u64::try_from(c.peak_pagefile_usage).ok())
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        None
    }
}

// ---------------------------------------------------------------------------
// The worker process
// ---------------------------------------------------------------------------

/// What the reader thread saw.
enum Incoming {
    /// A frame.
    Frame(FromWorker),
    /// Over a cap or not JSON.
    Breach,
    /// stdout ended (exit or crash).
    Closed,
}

/// Worker frames other than `JobDone` and `NeedOcr` are tiny (the worker's
/// own `frame_max` uses the same cap).
const MAX_SMALL_FRAME: usize = 4 * 1024;

/// How every worker frame starts: serde writes an internally tagged
/// enum's tag first.
const TAG_PREFIX: &[u8] = br#"{"type":""#;

/// The cap for a worker frame of type `tag`; `None` for an unknown type.
fn frame_cap(tag: &[u8]) -> Option<usize> {
    match tag {
        b"jobDone" => Some(MAX_JOB_DONE_FRAME),
        b"needOcr" => Some(MAX_FROM_WORKER_FRAME),
        b"ready" | b"pageStarted" | b"jobFailed" | b"unsupported" => Some(MAX_SMALL_FRAME),
        _ => None,
    }
}

/// Read one worker frame, checking its length against the cap **for its
/// type** before the rest of the body is read or parsed (review L1): only
/// a `NeedOcr` page may be large, so a compromised worker can't make the
/// desktop buffer and parse 40 MiB as a `JobDone` or a small frame.
fn read_worker_frame(r: &mut impl Read) -> Result<FromWorker, FrameError> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        let n = r.read(&mut len[got..])?;
        if n == 0 {
            return if got == 0 {
                Err(FrameError::Closed)
            } else {
                Err(FrameError::Io(std::io::ErrorKind::UnexpectedEof.into()))
            };
        }
        got += n;
    }
    let len = usize::try_from(u32::from_be_bytes(len)).unwrap_or(usize::MAX);
    if len > MAX_FROM_WORKER_FRAME {
        return Err(FrameError::TooLarge {
            len,
            max: MAX_FROM_WORKER_FRAME,
        });
    }
    // Enough for the prefix and the longest tag.
    let head = len.min(TAG_PREFIX.len() + 16);
    let mut body = vec![0u8; head];
    r.read_exact(&mut body)?;
    let cap = body
        .strip_prefix(TAG_PREFIX)
        .and_then(|rest| rest.iter().position(|&b| b == b'"').map(|end| &rest[..end]))
        .and_then(frame_cap)
        .ok_or(FrameError::Garbled)?;
    if len > cap {
        return Err(FrameError::TooLarge { len, max: cap });
    }
    body.resize(len, 0);
    r.read_exact(&mut body[head..])?;
    serde_json::from_slice(&body).map_err(|_| FrameError::Garbled)
}

/// Why a worker was stopped in the middle of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kill {
    Crashed,
    TimedOut,
    Memory,
    BadReply,
    Cancelled,
}

impl Kill {
    fn outcome(self) -> CouldNotRead {
        match self {
            Kill::Crashed => CouldNotRead::Crashed,
            Kill::TimedOut => CouldNotRead::TimedOut,
            Kill::Memory => CouldNotRead::TooMuchMemory,
            Kill::BadReply => CouldNotRead::BadReply,
            Kill::Cancelled => CouldNotRead::Cancelled,
        }
    }

    fn class(self) -> &'static str {
        match self {
            Kill::Crashed => "crashed",
            Kill::TimedOut => "timed out",
            Kill::Memory => "over its memory limit",
            Kill::BadReply => "bad reply",
            Kill::Cancelled => "cancelled",
        }
    }
}

struct Worker {
    child: Child,
    killer: TreeKiller,
    pid: u32,
    /// Encoded frames to write; dropping it closes the worker's stdin.
    tx: Option<Sender<Vec<u8>>>,
    rx: Receiver<Incoming>,
    served: u32,
    sys: System,
    /// Highest memory sampled.
    peak: u64,
    _dir: ScratchDir,
    _record: Option<ChildRecord>,
}

impl Worker {
    /// Start a worker and wait for `Ready`.
    ///
    /// On Linux the worker asks for `SIGKILL` when the thread that started
    /// it ends (`PR_SET_PDEATHSIG` follows the spawning *thread*, not the
    /// process). [`DocreadHost::read`] starts and stops its workers on the
    /// one thread it runs on; keep it that way: starting a worker on a
    /// short-lived helper thread would kill it when that thread ends.
    fn start(config: &DocreadConfig) -> Result<Worker, &'static str> {
        let dir = ScratchDir::new(&config.scratch, "w").map_err(|_| "no scratch folder")?;
        let mut env = scrubbed_env(&dir.0);
        // The worker exits at once when its parent is not this process
        // (a host that died before the worker set its death signal).
        env.push((
            "CL_DOCREAD_PARENT".into(),
            std::process::id().to_string().into(),
        ));
        if let Some(p) = std::env::var_os("CL_PDFIUM_PATH").filter(|p| Path::new(p).is_absolute()) {
            env.push(("CL_PDFIUM_PATH".into(), p));
        }
        if cfg!(feature = "test-support") {
            for (k, v) in &config.test_env {
                if k.starts_with("CL_DOCREAD_TEST_") {
                    env.push((k.into(), v.into()));
                }
            }
        }
        let mut cmd = Command::new(&config.worker_exe);
        cmd.env_clear()
            .envs(env)
            .current_dir(&dir.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        procs::prepare(&mut cmd);
        let mut child = cmd.spawn().map_err(|_| "could not start")?;
        let mut killer = TreeKiller::adopt(&child);
        let hard = config.limits.hard_cap(config.limits.worker_memory);
        // Fail closed (review M5): never a worker without its job. It has
        // not been given a document yet.
        if cfg!(windows) && !killer.confine(hard) {
            killer.terminate(&mut child, Duration::ZERO);
            return Err("not confined");
        }
        let pid = child.id();
        let record = ChildRecord::new(&config.scratch, &config.worker_exe, pid);
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("no pipes");
        };
        let (tx, wrx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || write_loop(stdin, &wrx));
        let (ftx, rx) = mpsc::channel();
        std::thread::spawn(move || read_loop(stdout, &ftx));
        std::thread::spawn(move || drain_stderr(stderr, "worker"));
        let mut w = Worker {
            child,
            killer,
            pid,
            tx: Some(tx),
            rx,
            served: 0,
            sys: System::new(),
            peak: 0,
            _dir: dir,
            _record: record,
        };
        match w.rx.recv_timeout(config.limits.ready) {
            Ok(Incoming::Frame(FromWorker::Ready { version })) if version == PROTOCOL_VERSION => {
                Ok(w)
            }
            Ok(Incoming::Frame(..) | Incoming::Breach) => {
                w.kill();
                Err("bad start")
            }
            Ok(Incoming::Closed) | Err(_) => {
                w.kill();
                Err("no ready")
            }
        }
    }

    fn send(&self, msg: &ToWorker) -> bool {
        let mut buf = Vec::new();
        if write_frame(&mut buf, msg, MAX_TO_WORKER_FRAME).is_err() {
            // `take_batch` and `templates_that_fit` keep batches within the
            // cap, so this is a host error, never the document's.
            tracing::warn!("docread: a message to the worker was over the frame limit");
            return false;
        }
        self.tx.as_ref().is_some_and(|tx| tx.send(buf).is_ok())
    }

    fn memory(&mut self) -> Option<u64> {
        let m = memory_of(&mut self.sys, self.pid);
        self.peak = self.peak.max(m.unwrap_or(0));
        m
    }

    /// The highest memory this worker used so far.
    fn peak(&self) -> u64 {
        self.peak.max(exact_peak(&self.child).unwrap_or(0))
    }

    fn kill(&mut self) {
        self.tx = None;
        self.killer.terminate(&mut self.child, Duration::ZERO);
    }

    /// Close stdin and give the worker a moment to leave, then kill.
    fn stop(mut self) {
        self.tx = None;
        let end = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        self.kill();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
}

fn write_loop(mut stdin: impl Write, rx: &Receiver<Vec<u8>>) {
    while let Ok(buf) = rx.recv() {
        if stdin.write_all(&buf).and_then(|()| stdin.flush()).is_err() {
            return;
        }
    }
}

fn read_loop(stdout: impl Read, tx: &Sender<Incoming>) {
    let mut r = std::io::BufReader::new(stdout);
    loop {
        let msg = match read_worker_frame(&mut r) {
            Ok(m) => Incoming::Frame(m),
            Err(FrameError::TooLarge { .. } | FrameError::Garbled) => Incoming::Breach,
            Err(FrameError::Closed | FrameError::Io(_)) => Incoming::Closed,
        };
        let last = !matches!(msg, Incoming::Frame(..));
        if tx.send(msg).is_err() || last {
            return;
        }
    }
}

/// Read a child's stderr to the end: the worker's fixed `docread: ` lines
/// are logged at debug level; everything else is only counted (it could
/// quote a document).
fn drain_stderr(stderr: impl Read, who: &'static str) {
    let mut r = std::io::BufReader::new(stderr);
    let mut line = Vec::new();
    let mut other = 0u32;
    let mut buf = [0u8; 4096];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            if b == b'\n' {
                log_line(&line, who, &mut other);
                line.clear();
            } else if line.len() < 256 {
                line.push(b);
            }
        }
    }
    log_line(&line, who, &mut other);
    if other > 0 {
        tracing::debug!(who, lines = other, "docread: other stderr lines");
    }
}

/// Every fixed line the worker writes to stderr (`clientledger-docread`;
/// a test checks its sources against this list). Nothing else a worker
/// writes is logged (review L7): a compromised one could otherwise copy
/// document words into the log.
const WORKER_LINES: &[&str] = &[
    "docread: limits: network namespace refused",
    "docread: limits: data limit refused",
    "docread: limits: cpu limit refused",
    "docread: limits: core limit refused",
    "docread: limits: dump protection refused",
    "docread: limits: parent watch refused",
    "docread: limits: host gone",
    "docread: outcome: pdf reader unavailable",
    "docread: outcome: reading over the frame limit",
    "docread: outcome: reading over a limit",
    "docread: outcome: ocr failed (unavailable)",
    "docread: outcome: ocr failed (timed out)",
    "docread: outcome: ocr failed (crashed)",
    "docread: outcome: ocr failed (bad image)",
    "docread: outcome: ocr failed (too much output)",
    "docread: protocol: unexpected frame during ocr",
    "docread: protocol: batch over a limit",
    "docread: protocol: ocr answer outside a job",
    "docread: protocol: bad frame",
];

/// Log a stderr line when it is one of [`WORKER_LINES`]: a refused limit
/// at warn (isolation is weaker on this machine, and the owner and CI can
/// see it), the rest at debug. Anything else is only counted.
fn log_line(line: &[u8], who: &'static str, other: &mut u32) {
    let text = String::from_utf8_lossy(line);
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    match WORKER_LINES.iter().find(|l| **l == text) {
        Some(l) if l.starts_with("docread: limits: ") => tracing::warn!(who, "{l}"),
        Some(l) => tracing::debug!(who, "{l}"),
        None => *other += 1,
    }
}

// ---------------------------------------------------------------------------
// Tesseract
// ---------------------------------------------------------------------------

/// The `tessdata_best` English model, installed beside the fast one
/// (`cargo xtask ocr-models`, Q-A03b).
const BEST_MODEL: &str = "engbest.traineddata";

/// Tesseract's arguments after the input, output and model folder; `best`
/// when [`BEST_MODEL`] is installed. `None`: the host can't serve the
/// mode (a second read without the best model).
///
/// Pages are read as one block of text (`--psm 6`), not with tesseract's
/// own page layout analysis (`--psm 3`): on bills and receipts its layout
/// pass cut right-aligned amount columns into separate blocks and misread
/// them (A03 WP-A2 round 2: seen photos' totals 88 → 95 %, scans' 93 →
/// 95 %). The rules build lines and columns from the word boxes anyway.
///
/// A page whose numbers read with low confidence is read again with both
/// English models (`eng+engbest`: tesseract re-reads the words the fast
/// model is unsure of with the best one and keeps the more confident
/// reading), and a single value is re-read with the best one alone (Q-A03b).
fn tesseract_args(mode: OcrMode, best: bool) -> Option<&'static [&'static str]> {
    Some(match (mode, best) {
        (OcrMode::Read, _) => &[
            "-l",
            "eng",
            "--oem",
            "1",
            "--psm",
            "6",
            "-c",
            "tessedit_create_tsv=1",
        ],
        (OcrMode::ReadBest, true) => &[
            "-l",
            "eng+engbest",
            "--oem",
            "1",
            "--psm",
            "6",
            "-c",
            "tessedit_create_tsv=1",
        ],
        (OcrMode::ReadBest, false) => return None,
        (OcrMode::Word, true) => &[
            "-l",
            "engbest",
            "--oem",
            "1",
            "--psm",
            "7",
            "-c",
            "tessedit_create_tsv=1",
        ],
        (OcrMode::Word, false) => &[
            "-l",
            "eng",
            "--oem",
            "1",
            "--psm",
            "7",
            "-c",
            "tessedit_create_tsv=1",
        ],
        // The orientation report (osd.traineddata), plain text.
        (OcrMode::Orient, _) => &["-l", "osd", "--psm", "0"],
    })
}

/// OCR one validated PGM page with tesseract: TSV text, or why not.
fn run_tesseract(
    tools: &OcrTools,
    (pgm, mode): (Vec<u8>, OcrMode),
    scratch: &Path,
    limits: &DocreadLimits,
    stop: [&AtomicBool; 2],
    peak: &mut u64,
) -> Result<String, OcrFailure> {
    let best = tools.tessdata.join(BEST_MODEL).is_file();
    let args = tesseract_args(mode, best).ok_or(OcrFailure::Unavailable)?;
    let dir = ScratchDir::new(scratch, "t").map_err(|_| OcrFailure::Unavailable)?;
    let mut cmd = Command::new(&tools.exe);
    cmd.arg("-")
        .arg("stdout")
        .arg("--tessdata-dir")
        .arg(&tools.tessdata)
        .args(args)
        .env_clear()
        .envs(scrubbed_env(&dir.0))
        .env("OMP_THREAD_LIMIT", "1")
        .current_dir(&dir.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    procs::prepare(&mut cmd);
    let mut child = cmd.spawn().map_err(|_| OcrFailure::Unavailable)?;
    let mut killer = TreeKiller::adopt(&child);
    let hard = limits.hard_cap(limits.tesseract_memory);
    // Before tesseract gets its page (it waits on stdin until then).
    if cfg!(windows) && !killer.confine(hard) {
        // Fail closed (review M5): never an unconfined tesseract.
        killer.terminate(&mut child, Duration::ZERO);
        tracing::warn!("docread: tesseract could not be confined");
        return Err(OcrFailure::Unavailable);
    }
    #[cfg(target_os = "linux")]
    {
        let cpu = limits.ocr_page.as_secs() + 1;
        if !procs::limit_child(child.id(), cpu, hard) {
            tracing::warn!("docread: tesseract limits refused");
        }
    }
    let _record = ChildRecord::new(scratch, &tools.exe, child.id());
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        killer.terminate(&mut child, Duration::ZERO);
        return Err(OcrFailure::Crashed);
    };
    std::thread::spawn(move || {
        let _ = stdin.write_all(&pgm);
    });
    let (otx, orx) = mpsc::channel::<Option<Vec<u8>>>();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let capped = stdout
            .take(u64::try_from(MAX_TSV_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut out)
            .is_ok();
        let _ = otx.send(capped.then_some(out));
    });
    std::thread::spawn(move || drain_stderr(stderr, "tesseract"));

    let started = Instant::now();
    let mut sys = System::new();
    let pid = child.id();
    let fail = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                *peak = (*peak).max(exact_peak(&child).unwrap_or(0));
                if !status.success() {
                    break Some(OcrFailure::Crashed);
                }
                break None;
            }
            Ok(None) => {}
            Err(_) => break Some(OcrFailure::Crashed),
        }
        if started.elapsed() > limits.ocr_page || stop.iter().any(|s| s.load(Ordering::Relaxed)) {
            break Some(OcrFailure::TimedOut);
        }
        let m = memory_of(&mut sys, pid);
        *peak = (*peak).max(m.unwrap_or(0));
        if m.is_some_and(|m| m > limits.tesseract_memory) {
            break Some(OcrFailure::Crashed);
        }
        std::thread::sleep(limits.poll.min(Duration::from_millis(20)));
    };
    if let Some(f) = fail {
        killer.terminate(&mut child, Duration::ZERO);
        return Err(f);
    }
    // The process has exited; its stdout ends now.
    let out = orx
        .recv_timeout(Duration::from_secs(5))
        .ok()
        .flatten()
        .ok_or(OcrFailure::Crashed)?;
    if out.len() > MAX_TSV_BYTES {
        return Err(OcrFailure::TooMuchOutput);
    }
    drop(dir);
    Ok(String::from_utf8_lossy(&out).into_owned())
}

// ---------------------------------------------------------------------------
// The host
// ---------------------------------------------------------------------------

/// How a batch on one worker ended.
enum BatchEnd {
    /// Every job in it finished.
    Done,
    /// The worker was killed during job `index` (into the batch).
    Killed(usize, Kill),
    /// The worker should be replaced (memory) after job `index` finished.
    Recycle(usize),
}

impl DocreadHost {
    /// A host for `config`.
    #[must_use]
    pub fn new(config: DocreadConfig) -> DocreadHost {
        DocreadHost {
            config,
            stats: DocreadStats::default(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether pictures and scanned pages can be read here.
    #[must_use]
    pub fn ocr_available(&self) -> bool {
        self.config.ocr.is_some()
    }

    /// Set it to stop the current [`DocreadHost::read`]: the worker is
    /// killed and every job not finished is
    /// [`CouldNotRead::Cancelled`]. Cleared at the start of each read.
    #[must_use]
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Counters since this host was made.
    #[must_use]
    pub fn stats(&self) -> DocreadStats {
        self.stats
    }

    /// Read `jobs` (blocking; see the module comment). One outcome per job,
    /// in order.
    pub fn read(&mut self, jobs: &[DocJob], today: CalendarDate) -> Vec<JobOutcome> {
        self.cancel.store(false, Ordering::Relaxed);
        self.read_jobs(jobs, &[], today)
    }

    /// Re-read one box of one page (blocking; protocol version 4): the
    /// worker cuts it out of the cleaned page and OCRs it (or takes the
    /// text layer's words). A box over 90 % of the page both ways is a
    /// read of that page. The answer is words only.
    pub fn read_region(&mut self, job: &RegionJob, today: CalendarDate) -> RegionOutcome {
        self.cancel.store(false, Ordering::Relaxed);
        self.region_jobs(job, today)
    }

    /// [`DocreadHost::read_region`] stopped by the caller's own flag (see
    /// [`DocreadHost::read_with_cancel`]).
    pub fn read_region_with_cancel(
        &mut self,
        job: &RegionJob,
        today: CalendarDate,
        cancel: Arc<AtomicBool>,
    ) -> RegionOutcome {
        let own = std::mem::replace(&mut self.cancel, cancel);
        let out = self.region_jobs(job, today);
        self.cancel = own;
        out
    }

    fn region_jobs(&mut self, job: &RegionJob, today: CalendarDate) -> RegionOutcome {
        if !region_ok(job.region) {
            return RegionOutcome::Refused;
        }
        let doc = DocJob {
            bytes: Arc::clone(&job.bytes),
            hint: JobHint::default(),
            templates: Vec::new(),
        };
        let out = self.read_jobs(std::slice::from_ref(&doc), &[Some(job.region)], today);
        match out.into_iter().next() {
            Some(JobOutcome::Read(r)) => {
                // Checked in `run_batch`: words only, this page, in the box.
                let words = r.words.into_iter().next().unwrap_or(PageWords {
                    page: job.region.page,
                    aspect: 0,
                    source: cl_docread::model::WordSource::TextLayer,
                    words: Vec::new(),
                });
                RegionOutcome::Words(words)
            }
            Some(JobOutcome::Failed(f)) => RegionOutcome::Failed(f),
            Some(JobOutcome::CouldNotRead(c)) => RegionOutcome::CouldNotRead(c),
            None => RegionOutcome::CouldNotRead(CouldNotRead::Cancelled),
        }
    }

    /// [`DocreadHost::read`] stopped by the caller's own flag, which is
    /// never cleared here: a flag set before the read starts cancels it at
    /// once (the reading service's Cancel, A03 review S2).
    pub fn read_with_cancel(
        &mut self,
        jobs: &[DocJob],
        today: CalendarDate,
        cancel: Arc<AtomicBool>,
    ) -> Vec<JobOutcome> {
        let own = std::mem::replace(&mut self.cancel, cancel);
        let out = self.read_jobs(jobs, &[], today);
        self.cancel = own;
        out
    }

    /// Read `jobs`; `regions[i]` (when present) makes job `i` a region
    /// re-read.
    fn read_jobs(
        &mut self,
        jobs: &[DocJob],
        regions: &[Option<Spot>],
        today: CalendarDate,
    ) -> Vec<JobOutcome> {
        let n = jobs.len();
        let mut outcomes: Vec<Option<JobOutcome>> = vec![None; n];
        let mut queue: VecDeque<usize> = VecDeque::new();
        for (i, j) in jobs.iter().enumerate() {
            match host_refusal(j) {
                Some(reason) => outcomes[i] = Some(JobOutcome::Failed(reason)),
                None => queue.push_back(i),
            }
        }
        let options = BatchOptions {
            ocr: self.config.ocr.is_some(),
            today,
        };
        let sending: Vec<Sending> = jobs.iter().map(plan_sending).collect();
        let mut kills = vec![0u8; n];
        let mut worker: Option<Worker> = None;
        let mut start_failures = 0u8;
        while !queue.is_empty() {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            let Some(w) = self.ready_worker(&mut worker) else {
                start_failures += 1;
                if start_failures >= self.config.limits.start_attempts {
                    for i in queue.drain(..) {
                        outcomes[i] = Some(JobOutcome::CouldNotRead(CouldNotRead::Unavailable));
                    }
                }
                continue;
            };
            start_failures = 0;
            let room = self.config.limits.recycle_jobs.saturating_sub(w.served);
            let batch = take_batch(&queue, jobs, &sending, room);
            let end = run_batch(
                w,
                &batch,
                (jobs, &sending, regions),
                options,
                &self.config,
                &mut self.stats,
                &self.cancel,
                &mut outcomes,
            );
            match end {
                BatchEnd::Done => {
                    queue.drain(..batch.len());
                }
                BatchEnd::Recycle(k) => {
                    queue.drain(..=k);
                    if let Some(w) = worker.take() {
                        w.stop();
                    }
                    self.stats.recycles += 1;
                }
                BatchEnd::Killed(k, why) => {
                    queue.drain(..k);
                    if let Some(mut w) = worker.take() {
                        w.kill();
                    }
                    self.stats.kills += 1;
                    tracing::info!(reason = why.class(), "docread: stopped the reading worker");
                    if why == Kill::Cancelled {
                        break;
                    }
                    let i = batch[k];
                    kills[i] += 1;
                    if kills[i] >= self.config.limits.kills_per_job {
                        outcomes[i] = Some(JobOutcome::CouldNotRead(why.outcome()));
                        queue.pop_front();
                    }
                }
            }
        }
        if let Some(w) = worker.take() {
            w.stop();
        }
        outcomes
            .into_iter()
            .map(|o| o.unwrap_or(JobOutcome::CouldNotRead(CouldNotRead::Cancelled)))
            .collect()
    }

    /// The worker to use next: the current one unless it has served its
    /// jobs (then it is replaced), else a new one. `None` when it could not
    /// be started.
    fn ready_worker<'w>(&mut self, worker: &'w mut Option<Worker>) -> Option<&'w mut Worker> {
        if worker
            .as_ref()
            // At least one job per worker, whatever the setting.
            .is_some_and(|w| w.served >= self.config.limits.recycle_jobs.max(1))
        {
            if let Some(w) = worker.take() {
                w.stop();
            }
            self.stats.recycles += 1;
        }
        if worker.is_none() {
            self.stats.workers_started += 1;
            match Worker::start(&self.config) {
                Ok(w) => *worker = Some(w),
                Err(why) => {
                    tracing::warn!(why, "docread: the reading worker did not start");
                    return None;
                }
            }
        }
        worker.as_mut()
    }
}

/// A file the host refuses without starting a worker.
fn host_refusal(job: &DocJob) -> Option<JobFailure> {
    if job.bytes.is_empty() {
        Some(JobFailure::Empty)
    } else if job.bytes.len() > MAX_DOCUMENT_BYTES {
        Some(JobFailure::TooLarge)
    } else {
        None
    }
}

/// Room in a batch frame for its jobs: the frame cap less the batch's own
/// JSON (type, options, brackets).
const BATCH_FRAME_BUDGET: usize = MAX_TO_WORKER_FRAME - 4 * 1024;

/// How one job goes into a batch frame (review L6): its templates that fit
/// one frame, and its encoded size with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sending {
    /// The first this many of the job's templates are sent.
    templates: usize,
    /// The job's bytes in the batch frame (exact JSON).
    frame_bytes: usize,
}

/// Size each job once, before any batch is serialized: its JSON without
/// document or templates, the document in base64, then templates in order
/// (at most `MAX_TEMPLATES`) while the job still fits a frame on its own.
/// So a job always fits one batch, whatever its templates hold (A04
/// learns them from documents); the rest are left out with a warning.
fn plan_sending(job: &DocJob) -> Sending {
    let bare = serde_json::to_vec(&Job {
        job: u32::MAX,
        bytes: Bytes(Vec::new()),
        hint: job.hint,
        templates: Vec::new(),
        // Room for a region's box.
        region: Some(Spot {
            page: u16::MAX,
            bbox: cl_docread::model::Bbox::PAGE,
        }),
    })
    .map_or(1024, |v| v.len());
    let mut size = bare + job.bytes.len().div_ceil(3) * 4 + 1;
    let mut templates = 0;
    for t in job.templates.iter().take(MAX_TEMPLATES) {
        // A comma between templates.
        let len = serde_json::to_vec(t).map_or(usize::MAX, |v| v.len() + 1);
        if size.saturating_add(len) > BATCH_FRAME_BUDGET {
            tracing::warn!(
                sent = templates,
                "docread: layout templates over the frame limit were left out"
            );
            break;
        }
        size += len;
        templates += 1;
    }
    Sending {
        templates,
        frame_bytes: size,
    }
}

/// The next jobs from the front of `queue` that fit one batch: at most
/// `room` (and the protocol's job cap), within the batch byte cap and the
/// frame cap (always at least one).
fn take_batch(
    queue: &VecDeque<usize>,
    jobs: &[DocJob],
    sending: &[Sending],
    room: u32,
) -> Vec<usize> {
    let max = MAX_JOBS_PER_BATCH.min(usize::try_from(room.max(1)).unwrap_or(1));
    let mut bytes = 0usize;
    let mut frame = 0usize;
    let mut out = Vec::new();
    for &i in queue {
        let len = jobs[i].bytes.len();
        let size = sending[i].frame_bytes;
        if out.len() == max
            || (!out.is_empty()
                && (bytes + len > MAX_BATCH_DOCUMENT_BYTES || frame + size > BATCH_FRAME_BUDGET))
        {
            break;
        }
        bytes += len;
        frame += size;
        out.push(i);
    }
    out
}

/// Where the current job stands.
struct JobClock {
    dispatched: Instant,
    deadline: Instant,
    pages: BTreeSet<u16>,
    /// Pages the job may read (`JobHint::max_pages`).
    max_pages: u16,
    /// The box of a region job.
    region: Option<Spot>,
}

impl JobClock {
    /// Whether the worker may start `page` now (see [`may_start`]).
    fn may_start(&self, page: u16) -> bool {
        may_start(self.region, &self.pages, page, self.max_pages)
    }

    /// A region job answered with more than the words in its box.
    fn wrong_region(&self, reading: &Reading) -> bool {
        self.region.is_some_and(|r| !is_region_reading(reading, r))
    }
}

/// Run one batch on `w`, filling `outcomes`.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_batch(
    w: &mut Worker,
    batch: &[usize],
    (jobs, sending, regions): (&[DocJob], &[Sending], &[Option<Spot>]),
    options: BatchOptions,
    config: &DocreadConfig,
    stats: &mut DocreadStats,
    cancel: &AtomicBool,
    outcomes: &mut [Option<JobOutcome>],
) -> BatchEnd {
    let limits = &config.limits;
    let sent: Vec<Job> = batch
        .iter()
        .map(|&i| Job {
            job: u32::try_from(i).unwrap_or(u32::MAX),
            bytes: Bytes(jobs[i].bytes.to_vec()),
            hint: jobs[i].hint,
            templates: jobs[i]
                .templates
                .iter()
                .take(sending[i].templates)
                .cloned()
                .collect(),
            region: regions.get(i).copied().flatten(),
        })
        .collect();
    let template_counts: Vec<usize> = sent.iter().map(|j| j.templates.len()).collect();
    if !w.send(&ToWorker::Batch {
        jobs: sent,
        options,
    }) {
        return BatchEnd::Killed(0, Kill::Crashed);
    }
    for (k, &i) in batch.iter().enumerate() {
        let id = u32::try_from(i).unwrap_or(u32::MAX);
        let now = Instant::now();
        let mut clock = JobClock {
            dispatched: now,
            deadline: now + limits.first_page,
            pages: BTreeSet::new(),
            max_pages: jobs[i].hint.max_pages(),
            region: regions.get(i).copied().flatten(),
        };
        let outcome = loop {
            if cancel.load(Ordering::Relaxed) {
                return BatchEnd::Killed(k, Kill::Cancelled);
            }
            let now = Instant::now();
            let ceiling = clock.dispatched + limits.job_ceiling(clock.pages.len());
            if now >= clock.deadline || now >= ceiling {
                return BatchEnd::Killed(k, Kill::TimedOut);
            }
            if w.memory().is_some_and(|m| m > limits.worker_memory) {
                return BatchEnd::Killed(k, Kill::Memory);
            }
            let wait = limits.poll.min(clock.deadline - now).min(ceiling - now);
            let msg = match w.rx.recv_timeout(wait) {
                Ok(Incoming::Frame(m)) => m,
                Ok(Incoming::Breach) => return BatchEnd::Killed(k, Kill::BadReply),
                Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    return BatchEnd::Killed(k, Kill::Crashed);
                }
                Err(RecvTimeoutError::Timeout) => continue,
            };
            match msg {
                FromWorker::PageStarted { job, page } if job == id && clock.may_start(page) => {
                    clock.pages.insert(page);
                    clock.deadline = Instant::now() + limits.per_page;
                }
                FromWorker::NeedOcr {
                    job,
                    page,
                    pgm,
                    mode,
                } if job == id && clock.pages.contains(&page) => {
                    let answer =
                        match ocr_watching(w, (id, page, mode), pgm.0, config, stats, cancel) {
                            Ok(a) => a,
                            Err(why) => return BatchEnd::Killed(k, why),
                        };
                    if !w.send(&answer) {
                        return BatchEnd::Killed(k, Kill::Crashed);
                    }
                    clock.deadline = Instant::now() + limits.per_page;
                }
                FromWorker::JobDone { job, reading } if job == id => {
                    if reading
                        .validate(template_counts[k], jobs[i].hint.statement)
                        .is_err()
                        || clock.wrong_region(&reading)
                    {
                        return BatchEnd::Killed(k, Kill::BadReply);
                    }
                    break JobOutcome::Read(reading);
                }
                FromWorker::JobFailed { job, reason } if job == id => {
                    break JobOutcome::Failed(reason);
                }
                _ => return BatchEnd::Killed(k, Kill::BadReply),
            }
        };
        outcomes[i] = Some(outcome);
        w.served += 1;
        stats.worker_peak_memory = stats.worker_peak_memory.max(w.peak());
        let more = k + 1 < batch.len();
        if more && w.memory().is_some_and(|m| m > limits.recycle_memory) {
            return BatchEnd::Recycle(k);
        }
        if !more && w.memory().is_some_and(|m| m > limits.recycle_memory) {
            // Nothing left in this batch: replace it before the next one.
            w.served = limits.recycle_jobs;
        }
    }
    BatchEnd::Done
}

/// Whether the worker may start `page` of a job: a page in range; for a
/// region job only the box's page, once (security review L2). Any other
/// start is a bad reply.
fn may_start(region: Option<Spot>, started: &BTreeSet<u16>, page: u16, max_pages: u16) -> bool {
    page < max_pages && region.is_none_or(|r| r.page == page && !started.contains(&page))
}

/// [`ocr_answer`] on a helper thread while this one keeps watching the
/// worker's memory (review L2): the worker waits for the answer, so an
/// honest one can't grow, but a compromised one could, and on macOS the
/// watchdog is its only memory limit. A breach stops tesseract and kills
/// the worker; cancelling stops tesseract (the caller then sees it).
fn ocr_watching(
    w: &mut Worker,
    (job, page, mode): (u32, u16, OcrMode),
    pgm: Vec<u8>,
    config: &DocreadConfig,
    stats: &mut DocreadStats,
    cancel: &AtomicBool,
) -> Result<ToWorker, Kill> {
    let stop = AtomicBool::new(false);
    let stop = &stop;
    std::thread::scope(|s| {
        let ocr =
            s.spawn(move || ocr_answer((job, page, mode), pgm, config, stats, [cancel, stop]));
        let mut breach = None;
        while !ocr.is_finished() {
            if breach.is_none() && w.memory().is_some_and(|m| m > config.limits.worker_memory) {
                breach = Some(Kill::Memory);
                stop.store(true, Ordering::Relaxed);
            }
            std::thread::sleep(config.limits.poll.min(Duration::from_millis(20)));
        }
        let answer = ocr.join().unwrap_or(ToWorker::OcrFailed {
            job,
            page,
            reason: OcrFailure::Crashed,
        });
        breach.map_or(Ok(answer), Err)
    })
}

/// The host's answer to a `NeedOcr`.
fn ocr_answer(
    (job, page, mode): (u32, u16, OcrMode),
    pgm: Vec<u8>,
    config: &DocreadConfig,
    stats: &mut DocreadStats,
    stop: [&AtomicBool; 2],
) -> ToWorker {
    let failed = |reason| ToWorker::OcrFailed { job, page, reason };
    if validate_pgm(&pgm).is_err() {
        stats.bad_images += 1;
        return failed(OcrFailure::BadImage);
    }
    let Some(tools) = &config.ocr else {
        return failed(OcrFailure::Unavailable);
    };
    // Orientation passes (a poor first read) are not pages read, and a
    // page with too little text for one is not an OCR failure.
    let reading = mode == OcrMode::Read;
    if reading {
        stats.ocr_pages += 1;
    }
    match mode {
        OcrMode::ReadBest => stats.ocr_best_pages += 1,
        OcrMode::Word => stats.ocr_rereads += 1,
        OcrMode::Read | OcrMode::Orient => {}
    }
    let mut peak = 0;
    let result = run_tesseract(
        tools,
        (pgm, mode),
        &config.scratch,
        &config.limits,
        stop,
        &mut peak,
    );
    stats.tesseract_peak_memory = stats.tesseract_peak_memory.max(peak);
    match result {
        Ok(tsv) => ToWorker::OcrResult {
            job,
            page,
            tsv: Tsv(tsv),
        },
        Err(reason) => {
            if reading {
                stats.ocr_failures += 1;
                tracing::info!(reason = ?reason, "docread: a page could not be read with OCR");
            }
            failed(reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_docread::model::MAX_PAGES;

    #[test]
    fn the_best_model_is_used_only_for_second_reads() {
        let lang = |mode, best| {
            tesseract_args(mode, best).map(|a| {
                let i = a.iter().position(|x| *x == "-l").unwrap();
                (a[i + 1], a.windows(2).any(|w| w == ["--psm", "7"]))
            })
        };
        // The first read is always the fast model alone, one block.
        assert_eq!(lang(OcrMode::Read, true), Some(("eng", false)));
        assert_eq!(lang(OcrMode::Read, false), Some(("eng", false)));
        assert!(
            tesseract_args(OcrMode::Read, true)
                .unwrap()
                .windows(2)
                .any(|w| w == ["--psm", "6"])
        );
        assert_eq!(lang(OcrMode::ReadBest, true), Some(("eng+engbest", false)));
        assert_eq!(lang(OcrMode::ReadBest, false), None, "no best model");
        assert_eq!(lang(OcrMode::Word, true), Some(("engbest", true)));
        assert_eq!(lang(OcrMode::Word, false), Some(("eng", true)));
        assert_eq!(lang(OcrMode::Orient, true), Some(("osd", false)));
    }

    fn job(len: usize) -> DocJob {
        DocJob {
            bytes: vec![1; len].into(),
            hint: JobHint::default(),
            templates: Vec::new(),
        }
    }

    fn plans(jobs: &[DocJob]) -> Vec<Sending> {
        jobs.iter().map(plan_sending).collect()
    }

    #[test]
    fn batches_keep_within_the_protocol_caps() {
        let jobs: Vec<DocJob> = (0..45).map(|_| job(10)).collect();
        let q: VecDeque<usize> = (0..45).collect();
        let s = plans(&jobs);
        assert_eq!(take_batch(&q, &jobs, &s, 20).len(), MAX_JOBS_PER_BATCH);
        assert_eq!(take_batch(&q, &jobs, &s, 3), [0, 1, 2]);
        let big: Vec<DocJob> = (0..6).map(|_| job(MAX_DOCUMENT_BYTES)).collect();
        let q: VecDeque<usize> = (0..6).collect();
        let s = plans(&big);
        assert_eq!(take_batch(&q, &big, &s, 20).len(), 4);
        // A worker with no room left still gets one job (it is replaced first).
        assert_eq!(take_batch(&q, &big, &s, 0).len(), 1);
    }

    /// A template far over the protocol's own caps (about 600 KB): the
    /// host must cope with whatever the company's templates hold.
    fn huge_template() -> Template {
        Template {
            fingerprint: cl_docread::model::Fingerprint {
                tokens: (0..300).map(|i| format!("{i:0>2000}")).collect(),
                aspect: 12_941,
            },
            fields: Vec::new(),
            line_table: None,
            samples: 1,
        }
    }

    #[test]
    fn big_templates_never_overflow_a_batch_frame() {
        // Review L6: a 10 MB document whose templates alone are over the
        // frame cap, then a 1 MB one.
        let doc = DocJob {
            bytes: vec![7; MAX_DOCUMENT_BYTES].into(),
            hint: JobHint::default(),
            templates: vec![huge_template(); 120],
        };
        let jobs = vec![doc, job(1024 * 1024)];
        let s = plans(&jobs);
        assert!(s[0].templates < 120, "{:?}", s[0]);
        assert!(s[0].templates > 0);
        assert!(s[0].frame_bytes <= BATCH_FRAME_BUDGET);
        assert_eq!(s[1].templates, 0);
        // The second job goes in the next batch, and the first batch
        // really fits a frame.
        let q: VecDeque<usize> = (0..2).collect();
        assert_eq!(take_batch(&q, &jobs, &s, 20), [0]);
        assert_eq!(
            take_batch(&q.range(1..).copied().collect(), &jobs, &s, 20),
            [1]
        );
        let batch = ToWorker::Batch {
            jobs: vec![Job {
                job: 0,
                bytes: Bytes(jobs[0].bytes.to_vec()),
                hint: jobs[0].hint,
                templates: jobs[0].templates[..s[0].templates].to_vec(),
                region: None,
            }],
            options: BatchOptions {
                ocr: true,
                today: CalendarDate::from_ymd(2026, 10, 26).unwrap(),
            },
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &batch, MAX_TO_WORKER_FRAME).unwrap();
        // The estimate is exact to within the batch's own JSON.
        let body = buf.len() - 4;
        assert!(body <= MAX_TO_WORKER_FRAME);
        assert!(
            body.abs_diff(s[0].frame_bytes) < 4 * 1024,
            "{body} vs {:?}",
            s[0]
        );
    }

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut v = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn each_worker_frame_type_has_its_own_cap_checked_before_parsing() {
        // Review L1.
        let ok = |m: &FromWorker| {
            let mut buf = Vec::new();
            write_frame(&mut buf, m, MAX_FROM_WORKER_FRAME).unwrap();
            read_worker_frame(&mut buf.as_slice()).unwrap()
        };
        let ready = FromWorker::Ready { version: 1 };
        assert_eq!(ok(&ready), ready);
        let failed = FromWorker::JobFailed {
            job: 3,
            reason: JobFailure::Unreadable,
        };
        assert_eq!(ok(&failed), failed);
        // A small type padded past 4 KiB is refused before the body is read.
        let mut padded = br#"{"type":"ready","version":1"#.to_vec();
        padded.extend(std::iter::repeat_n(b' ', 5000));
        padded.push(b'}');
        let only_head = &frame(&padded)[..4 + 40];
        assert!(matches!(
            read_worker_frame(&mut &only_head[..]),
            Err(FrameError::TooLarge { max: 4096, .. })
        ));
        // A `JobDone` over the cap likewise (only its head is sent here).
        let len = u32::try_from(MAX_JOB_DONE_FRAME + 1).unwrap();
        let mut big = len.to_be_bytes().to_vec();
        big.extend_from_slice(br#"{"type":"jobDone","job":1,"#);
        assert!(matches!(
            read_worker_frame(&mut big.as_slice()),
            Err(FrameError::TooLarge { .. })
        ));
        // Over the frame cap, an unknown type, or not our JSON at all.
        let len = u32::try_from(MAX_FROM_WORKER_FRAME + 1).unwrap();
        assert!(matches!(
            read_worker_frame(&mut len.to_be_bytes().as_slice()),
            Err(FrameError::TooLarge { .. })
        ));
        for body in [
            &br#"{"type":"render","job":1}"#[..],
            b"nope!",
            br#"{ "type":"ready","version":1}"#,
        ] {
            assert!(matches!(
                read_worker_frame(&mut frame(body).as_slice()),
                Err(FrameError::Garbled)
            ));
        }
        // A clean end, and an end inside a frame.
        assert!(matches!(
            read_worker_frame(&mut &b""[..]),
            Err(FrameError::Closed)
        ));
        assert!(matches!(
            read_worker_frame(&mut &frame(b"{\"type\":\"ready\"}")[..10]),
            Err(FrameError::Io(_))
        ));
    }

    #[test]
    fn leftover_scratch_folders_are_recognised_by_name() {
        let me = std::process::id();
        let other = me.wrapping_add(1);
        assert!(is_leftover_scratch(&format!("w-{other}-0"), me));
        assert!(is_leftover_scratch(&format!("t-{other}-17"), me));
        assert!(!is_leftover_scratch(&format!("w-{me}-0"), me), "ours");
        assert!(!is_leftover_scratch(".pids", me));
        assert!(!is_leftover_scratch(&format!("x-{other}-0"), me));
        assert!(!is_leftover_scratch(&format!("w-{other}-0-1"), me));
        assert!(!is_leftover_scratch(&format!("w-{other}"), me));
    }

    /// A long-lived stand-in named like tesseract (a copy of `ping` or
    /// `sleep`), so the sweep's name check accepts it.
    fn fake_tesseract(dir: &Path) -> (PathBuf, std::process::Child) {
        let exe = dir.join(crate::paths::tesseract_exe_name());
        let mut cmd = if cfg!(windows) {
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
            std::fs::copy(Path::new(&root).join(r"System32\PING.EXE"), &exe).unwrap();
            let mut c = Command::new(&exe);
            c.args(["-n", "60", "127.0.0.1"]);
            c
        } else {
            std::fs::copy("/bin/sleep", &exe).unwrap();
            let mut c = Command::new(&exe);
            c.arg("60");
            c
        };
        cmd.stdout(Stdio::null());
        // A test thread forking at the same moment can still hold the copy's
        // write handle; Linux then refuses to run it ("Text file busy").
        let mut tries = 0;
        let child = loop {
            match cmd.spawn() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 50 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                other => break other.unwrap(),
            }
        };
        (exe.clone(), child)
    }

    #[test]
    fn a_crashed_hosts_readers_and_folders_are_swept() {
        let root =
            std::env::temp_dir().join(format!("cl-host-docread-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let scratch = root.join("docread");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let me = std::process::id();
        let other = me.wrapping_add(1);
        for d in [
            format!("w-{other}-0"),
            format!("t-{other}-3"),
            format!("w-{me}-9"),
            "keep".to_string(),
        ] {
            std::fs::create_dir_all(scratch.join(d)).unwrap();
        }
        let (exe, mut child) = fake_tesseract(&bin);
        std::thread::sleep(Duration::from_millis(300));
        // Recorded by "another host": rename our record to its pid.
        let record = ChildRecord::new(&scratch, &exe, child.id()).unwrap();
        let theirs = records_dir(&scratch).join(format!("{other}-{}", child.id()));
        std::fs::rename(&record.0, &theirs).unwrap();
        drop(record);
        // One of ours (kept), one for a program that is not a reader, and
        // one whose start time does not match (a recycled pid): never killed.
        let ours = ChildRecord::new(&scratch, &exe, child.id()).unwrap();
        let wrong_name = records_dir(&scratch).join(format!("{other}-1"));
        std::fs::write(
            &wrong_name,
            format!("{}\n0\n{}\n", child.id(), bin.join("other").display()),
        )
        .unwrap();
        let swept = sweep(&scratch);
        let ended = (0..60).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            matches!(child.try_wait(), Ok(Some(_)))
        });
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the leftover reader should have been stopped");
        assert_eq!(
            swept,
            DocreadSwept {
                stopped: 1,
                folders: 2
            }
        );
        assert!(!theirs.exists() && !wrong_name.exists());
        assert!(ours.0.exists(), "this process's records are kept");
        assert!(scratch.join(format!("w-{me}-9")).exists());
        assert!(scratch.join("keep").exists());
        drop(ours);
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_record_for_a_recycled_pid_kills_nothing() {
        let root =
            std::env::temp_dir().join(format!("cl-host-docread-recycled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let scratch = root.join("docread");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let (exe, mut child) = fake_tesseract(&bin);
        std::thread::sleep(Duration::from_millis(300));
        let dir = records_dir(&scratch);
        create_private_dir(&dir).unwrap();
        let other = std::process::id().wrapping_add(1);
        std::fs::write(
            dir.join(format!("{other}-{}", child.id())),
            format!("{}\n1\n{}\n", child.id(), exe.display()),
        )
        .unwrap();
        assert_eq!(sweep(&scratch).stopped, 0);
        assert!(matches!(child.try_wait(), Ok(None)), "still running");
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Review M5: on Windows a worker whose job can't be set up is stopped
    /// before it is given anything.
    #[cfg(windows)]
    #[test]
    fn a_worker_that_cannot_be_confined_is_not_run() {
        let root =
            std::env::temp_dir().join(format!("cl-host-docread-confine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let root_os = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let config = DocreadConfig {
            worker_exe: Path::new(&root_os).join(r"System32\PING.EXE"),
            ocr: None,
            scratch: root.join("docread"),
            limits: DocreadLimits::default(),
            test_env: Vec::new(),
        };
        procs::FAIL_CONFINE.with(|f| f.set(true));
        let started = Worker::start(&config);
        procs::FAIL_CONFINE.with(|f| f.set(false));
        assert_eq!(started.err(), Some("not confined"));
        // Its scratch folder and record are gone with it.
        let left = std::fs::read_dir(root.join("docread")).map_or(0, |d| {
            d.flatten().filter(|e| e.file_name() != ".pids").count()
        });
        assert_eq!(left, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn every_line_the_worker_writes_is_in_the_fixed_list() {
        // Review L7: the host logs only exact lines; this keeps the list in
        // step with the worker's sources.
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/clientledger-docread/src");
        let mut found = 0;
        for e in std::fs::read_dir(&src).unwrap().flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap();
            // The shipped code, not its tests.
            let text = text.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
            for part in text.split("\"docread: ").skip(1) {
                let line = format!("docread: {}", part.split('"').next().unwrap());
                found += 1;
                if let Some((head, _)) = line.split_once("{}") {
                    assert!(
                        WORKER_LINES.iter().any(|l| l.starts_with(head)),
                        "{line} (in {:?})",
                        e.file_name()
                    );
                } else {
                    assert!(
                        WORKER_LINES.contains(&line.as_str()),
                        "{line} (in {:?})",
                        e.file_name()
                    );
                }
            }
        }
        assert!(found >= 10, "{found}");
    }

    #[test]
    fn the_job_ceiling_grows_per_page_up_to_twenty() {
        let l = DocreadLimits::default();
        assert_eq!(l.job_ceiling(0), Duration::from_secs(60));
        assert_eq!(l.job_ceiling(3), Duration::from_secs(120));
        assert_eq!(l.job_ceiling(50), Duration::from_secs(630));
        assert_eq!(l.hard_cap(l.worker_memory), 1280 * MIB);
    }

    #[test]
    fn only_the_workers_fixed_lines_are_logged() {
        let mut other = 0;
        log_line(b"docread: limits: data limit refused", "w", &mut other);
        log_line(b"docread: protocol: bad frame\r", "w", &mut other);
        log_line(b"docread: Riverbend 544.82", "w", &mut other);
        // Lowercase words in the old pattern's shape are no longer enough.
        log_line(b"docread: outcome: riverbend paving", "w", &mut other);
        log_line(b"thread 'main' panicked at C:\\x\\y.rs", "w", &mut other);
        log_line(b"   ", "w", &mut other);
        assert_eq!(other, 3);
    }

    /// Security review L2: a region job's worker may start only its page,
    /// once; a whole read any page in range.
    #[test]
    fn a_region_job_may_start_only_its_page_once() {
        let r = Some(Spot {
            page: 2,
            bbox: cl_docread::model::Bbox::PAGE,
        });
        let mut started = BTreeSet::new();
        assert!(may_start(r, &started, 2, MAX_PAGES));
        assert!(!may_start(r, &started, 1, MAX_PAGES));
        assert!(!may_start(r, &started, 3, MAX_PAGES));
        started.insert(2);
        assert!(!may_start(r, &started, 2, MAX_PAGES), "twice");
        assert!(may_start(None, &started, 2, MAX_PAGES));
        assert!(may_start(None, &started, 7, MAX_PAGES));
        assert!(!may_start(None, &started, MAX_PAGES, MAX_PAGES));
        // A statement job reads twenty pages (A06).
        let statement = JobHint::statement().max_pages();
        assert!(may_start(None, &started, MAX_PAGES, statement));
        assert!(!may_start(None, &started, MAX_STATEMENT_PAGES, statement));
    }

    #[test]
    fn debug_shows_no_document_bytes() {
        let j = DocJob {
            bytes: b"Riverbend".to_vec().into(),
            hint: JobHint::default(),
            templates: Vec::new(),
        };
        assert!(!format!("{j:?}").contains("Riverbend"));
    }

    #[test]
    #[cfg(windows)]
    fn the_exact_peak_is_read_from_the_process() {
        let mut child = Command::new("cmd")
            .args(["/c", "exit"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let live = exact_peak(&child);
        let _ = child.wait();
        let after = exact_peak(&child);
        assert!(live.is_some_and(|b| b > 0), "{live:?}");
        assert!(after.is_some_and(|b| b > 0), "{after:?}");
    }

    #[test]
    fn children_get_a_scrubbed_environment() {
        let env = scrubbed_env(Path::new("/tmp/x"));
        let keys: Vec<String> = env
            .iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        for k in ["TEMP", "TMP", "TMPDIR", "PATH"] {
            assert!(keys.iter().any(|x| x == k), "{k}");
        }
        assert!(!keys.iter().any(|k| k.starts_with("CL_") || k == "RUST_LOG"));
    }
}
