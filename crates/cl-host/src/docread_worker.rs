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
//! Logs carry outcome classes and counts only: never a file name, a path
//! inside a document, text or an amount. Worker stderr lines are logged
//! only when they are the worker's fixed `docread: ` lines.

use crate::paths::create_private_dir;
use crate::procs::{self, TreeKiller};
use cl_core::CalendarDate;
use cl_docread::model::{MAX_PAGES, Reading, Template};
use cl_docread::protocol::{
    BatchOptions, Bytes, FrameError, FromWorker, Job, JobFailure, JobHint,
    MAX_BATCH_DOCUMENT_BYTES, MAX_DOCUMENT_BYTES, MAX_FROM_WORKER_FRAME, MAX_JOB_DONE_FRAME,
    MAX_JOBS_PER_BATCH, MAX_TEMPLATES, MAX_TO_WORKER_FRAME, MAX_TSV_BYTES, OcrFailure,
    PROTOCOL_VERSION, ToWorker, Tsv, read_frame, validate_pgm, write_frame,
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

/// Time and memory limits (plan D2, Q-A03c). **Provisional** until step
/// 12a measures the worker and tesseract; WP-C sets the final values here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocreadLimits {
    /// Start → `Ready`.
    pub ready: Duration,
    /// Job dispatch → its first `PageStarted` (a load that hangs).
    pub first_page: Duration,
    /// Each page: from its `PageStarted`, or from the host's OCR answer.
    pub per_page: Duration,
    /// The job ceiling's fixed part: ceiling = this + pages started (at
    /// most ten) × `per_page`.
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
            tesseract_memory: 512 * MIB,
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
        let n = u32::try_from(pages.clamp(1, usize::from(MAX_PAGES))).unwrap_or(1);
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

/// One document to read. File names never cross to the worker.
#[derive(Clone, PartialEq, Eq)]
pub struct DocJob {
    pub bytes: Vec<u8>,
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
    pub bad_images: u32,
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

/// A fresh owner-only folder below `root`, removed on drop.
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

/// Counts bytes read, so a frame's body length is known after
/// `read_frame` (the `JobDone` cap is smaller than the frame cap).
struct Counting<R> {
    inner: R,
    count: usize,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count += n;
        Ok(n)
    }
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
    _dir: ScratchDir,
}

impl Worker {
    /// Start a worker and wait for `Ready`.
    fn start(config: &DocreadConfig) -> Result<Worker, &'static str> {
        let dir = ScratchDir::new(&config.scratch, "w").map_err(|_| "no scratch folder")?;
        let mut env = scrubbed_env(&dir.0);
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
        let killer = TreeKiller::adopt(&child);
        let hard = config.limits.hard_cap(config.limits.worker_memory);
        killer.limit_memory(hard);
        let pid = child.id();
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
            _dir: dir,
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
            return false;
        }
        self.tx.as_ref().is_some_and(|tx| tx.send(buf).is_ok())
    }

    fn memory(&mut self) -> Option<u64> {
        memory_of(&mut self.sys, self.pid)
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
    let mut r = Counting {
        inner: std::io::BufReader::new(stdout),
        count: 0,
    };
    loop {
        let before = r.count;
        let msg = match read_frame::<_, FromWorker>(&mut r, MAX_FROM_WORKER_FRAME) {
            Ok(m) => {
                let body = (r.count - before).saturating_sub(4);
                if matches!(m, FromWorker::JobDone { .. }) && body > MAX_JOB_DONE_FRAME {
                    Incoming::Breach
                } else {
                    Incoming::Frame(m)
                }
            }
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

fn log_line(line: &[u8], who: &'static str, other: &mut u32) {
    let text = String::from_utf8_lossy(line);
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let fixed = text.starts_with("docread: ")
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b" :-()".contains(&b));
    if fixed {
        tracing::debug!(who, "{text}");
    } else {
        *other += 1;
    }
}

// ---------------------------------------------------------------------------
// Tesseract
// ---------------------------------------------------------------------------

/// OCR one validated PGM page with tesseract: TSV text, or why not.
fn run_tesseract(
    tools: &OcrTools,
    pgm: Vec<u8>,
    scratch: &Path,
    limits: &DocreadLimits,
    cancel: &AtomicBool,
) -> Result<String, OcrFailure> {
    let dir = ScratchDir::new(scratch, "t").map_err(|_| OcrFailure::Unavailable)?;
    let mut cmd = Command::new(&tools.exe);
    cmd.arg("-")
        .arg("stdout")
        .arg("--tessdata-dir")
        .arg(&tools.tessdata)
        .args([
            "-l",
            "eng",
            "--oem",
            "1",
            "--psm",
            "3",
            "-c",
            "tessedit_create_tsv=1",
        ])
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
    killer.limit_memory(limits.hard_cap(limits.tesseract_memory));
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
                if !status.success() {
                    break Some(OcrFailure::Crashed);
                }
                break None;
            }
            Ok(None) => {}
            Err(_) => break Some(OcrFailure::Crashed),
        }
        if started.elapsed() > limits.ocr_page || cancel.load(Ordering::Relaxed) {
            break Some(OcrFailure::TimedOut);
        }
        if memory_of(&mut sys, pid).is_some_and(|m| m > limits.tesseract_memory) {
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
            let batch = take_batch(&queue, jobs, self.config.limits.recycle_jobs - w.served);
            let end = run_batch(
                w,
                &batch,
                jobs,
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
            .is_some_and(|w| w.served >= self.config.limits.recycle_jobs)
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

/// The next jobs from the front of `queue` that fit one batch: at most
/// `room` (and the protocol's job cap), within the batch byte cap (always
/// at least one).
fn take_batch(queue: &VecDeque<usize>, jobs: &[DocJob], room: u32) -> Vec<usize> {
    let max = MAX_JOBS_PER_BATCH.min(usize::try_from(room.max(1)).unwrap_or(1));
    let mut bytes = 0usize;
    let mut out = Vec::new();
    for &i in queue {
        let len = jobs[i].bytes.len();
        if out.len() == max || (!out.is_empty() && bytes + len > MAX_BATCH_DOCUMENT_BYTES) {
            break;
        }
        bytes += len;
        out.push(i);
    }
    out
}

/// Where the current job stands.
struct JobClock {
    dispatched: Instant,
    deadline: Instant,
    pages: BTreeSet<u16>,
}

/// Run one batch on `w`, filling `outcomes`.
#[allow(clippy::too_many_arguments)]
fn run_batch(
    w: &mut Worker,
    batch: &[usize],
    jobs: &[DocJob],
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
            bytes: Bytes(jobs[i].bytes.clone()),
            hint: jobs[i].hint,
            templates: jobs[i]
                .templates
                .iter()
                .take(MAX_TEMPLATES)
                .cloned()
                .collect(),
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
                FromWorker::PageStarted { job, page } if job == id && page < MAX_PAGES => {
                    clock.pages.insert(page);
                    clock.deadline = Instant::now() + limits.per_page;
                }
                FromWorker::NeedOcr { job, page, pgm }
                    if job == id && clock.pages.contains(&page) =>
                {
                    let answer = ocr_answer(id, page, pgm.0, config, stats, cancel);
                    if !w.send(&answer) {
                        return BatchEnd::Killed(k, Kill::Crashed);
                    }
                    clock.deadline = Instant::now() + limits.per_page;
                }
                FromWorker::JobDone { job, reading } if job == id => {
                    if reading.validate(template_counts[k]).is_err() {
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

/// The host's answer to a `NeedOcr`.
fn ocr_answer(
    job: u32,
    page: u16,
    pgm: Vec<u8>,
    config: &DocreadConfig,
    stats: &mut DocreadStats,
    cancel: &AtomicBool,
) -> ToWorker {
    let failed = |reason| ToWorker::OcrFailed { job, page, reason };
    if validate_pgm(&pgm).is_err() {
        stats.bad_images += 1;
        return failed(OcrFailure::BadImage);
    }
    let Some(tools) = &config.ocr else {
        return failed(OcrFailure::Unavailable);
    };
    stats.ocr_pages += 1;
    match run_tesseract(tools, pgm, &config.scratch, &config.limits, cancel) {
        Ok(tsv) => ToWorker::OcrResult {
            job,
            page,
            tsv: Tsv(tsv),
        },
        Err(reason) => {
            stats.ocr_failures += 1;
            tracing::info!(reason = ?reason, "docread: a page could not be read with OCR");
            failed(reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(len: usize) -> DocJob {
        DocJob {
            bytes: vec![1; len],
            hint: JobHint::default(),
            templates: Vec::new(),
        }
    }

    #[test]
    fn batches_keep_within_the_protocol_caps() {
        let jobs: Vec<DocJob> = (0..45).map(|_| job(10)).collect();
        let q: VecDeque<usize> = (0..45).collect();
        assert_eq!(take_batch(&q, &jobs, 20).len(), MAX_JOBS_PER_BATCH);
        assert_eq!(take_batch(&q, &jobs, 3), [0, 1, 2]);
        let big: Vec<DocJob> = (0..6).map(|_| job(MAX_DOCUMENT_BYTES)).collect();
        let q: VecDeque<usize> = (0..6).collect();
        assert_eq!(take_batch(&q, &big, 20).len(), 4);
        // A worker with no room left still gets one job (it is replaced first).
        assert_eq!(take_batch(&q, &big, 0).len(), 1);
    }

    #[test]
    fn the_job_ceiling_grows_per_page_up_to_ten() {
        let l = DocreadLimits::default();
        assert_eq!(l.job_ceiling(0), Duration::from_secs(60));
        assert_eq!(l.job_ceiling(3), Duration::from_secs(120));
        assert_eq!(l.job_ceiling(50), Duration::from_secs(330));
        assert_eq!(l.hard_cap(l.worker_memory), 1280 * MIB);
    }

    #[test]
    fn only_the_workers_fixed_lines_are_logged() {
        let mut other = 0;
        log_line(b"docread: limits: data limit refused", "w", &mut other);
        log_line(b"docread: Riverbend 544.82", "w", &mut other);
        log_line(b"thread 'main' panicked at C:\\x\\y.rs", "w", &mut other);
        log_line(b"   ", "w", &mut other);
        assert_eq!(other, 2);
    }

    #[test]
    fn debug_shows_no_document_bytes() {
        let j = DocJob {
            bytes: b"Riverbend".to_vec(),
            hint: JobHint::default(),
            templates: Vec::new(),
        };
        assert!(!format!("{j:?}").contains("Riverbend"));
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
