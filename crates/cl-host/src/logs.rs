//! The host's own log, rotation, and retention (Q-118).
//!
//! * [`HostLog`] appends one line per event to `host.log` and rotates it by
//!   size (`host.log.1`, `host.log.2`, ...).
//! * The engine writes and rotates its own `engine.log` (`CL_LOG_FILE`,
//!   `CL_LOG_MAX_BYTES`, `CL_LOG_KEEP`); the same naming applies.
//! * [`prune_logs`] enforces the retention rule on the whole logs folder:
//!   rotated files older than 30 days go, and while the folder is over
//!   100 MB the oldest rotated files go, whichever limit is hit first. The
//!   active files (`*.log`) are never deleted.
//!
//! Files are opened the way the standard library does on Windows, with
//! `FILE_SHARE_DELETE`, so a rotated file that a log viewer (or the engine)
//! still has open can be renamed and deleted.
//!
//! Messages never contain amounts, names, memo text, or secrets (CLAUDE.md
//! logging rule); callers pass text that is already scrubbed.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Severity of a log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Normal progress.
    Info,
    /// Something recovered or degraded.
    Warn,
    /// Something failed.
    Error,
}

impl Level {
    fn word(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// Rotation and retention limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogPolicy {
    /// Rotated files older than this are deleted (Q-118: 30 days).
    pub max_age: Duration,
    /// The logs folder may hold at most this many bytes (Q-118: 100 MB).
    pub max_total_bytes: u64,
    /// `host.log` is rotated when it reaches this size.
    pub rotate_bytes: u64,
    /// Rotated host logs kept by rotation itself (`host.log.1` .. `.N`).
    pub keep_rotated: u32,
}

impl Default for LogPolicy {
    fn default() -> Self {
        LogPolicy {
            max_age: Duration::from_hours(30 * 24),
            max_total_bytes: 100 * 1024 * 1024,
            rotate_bytes: 5 * 1024 * 1024,
            keep_rotated: 3,
        }
    }
}

#[derive(Debug)]
struct Active {
    file: Option<File>,
    size: u64,
}

/// An append-only, size-rotated log file shared between threads.
#[derive(Debug)]
pub struct HostLog {
    path: PathBuf,
    policy: LogPolicy,
    active: Mutex<Active>,
}

impl HostLog {
    /// Open (creating if needed) the log at `path` with the default policy.
    /// Failure to open is not an error: the host must run even when its log
    /// folder is unwritable, so lines are then only passed to `tracing`.
    #[must_use]
    pub fn open(path: impl Into<PathBuf>) -> HostLog {
        Self::open_with(path, LogPolicy::default())
    }

    /// [`HostLog::open`] with explicit limits.
    #[must_use]
    pub fn open_with(path: impl Into<PathBuf>, policy: LogPolicy) -> HostLog {
        let path = path.into();
        let file = open_append(&path);
        let size = file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
        HostLog {
            path,
            policy,
            active: Mutex::new(Active { file, size }),
        }
    }

    /// Where the log is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one line (and forward it to `tracing`).
    pub fn log(&self, level: Level, message: &str) {
        match level {
            Level::Info => tracing::info!("{message}"),
            Level::Warn => tracing::warn!("{message}"),
            Level::Error => tracing::error!("{message}"),
        }
        let line = format!("{} {} {}\n", utc_now(), level.word(), one_line(message));
        let Ok(mut a) = self.active.lock() else {
            return;
        };
        if a.size >= self.policy.rotate_bytes {
            self.rotate(&mut a);
        }
        if let Some(f) = a.file.as_mut()
            && f.write_all(line.as_bytes()).is_ok()
        {
            a.size += line.len() as u64;
        }
    }

    /// [`HostLog::log`] at `Info`.
    pub fn info(&self, message: &str) {
        self.log(Level::Info, message);
    }

    /// [`HostLog::log`] at `Warn`.
    pub fn warn(&self, message: &str) {
        self.log(Level::Warn, message);
    }

    /// [`HostLog::log`] at `Error`.
    pub fn error(&self, message: &str) {
        self.log(Level::Error, message);
    }

    /// `host.log` becomes `host.log.1`, `.1` becomes `.2`, and so on; the
    /// oldest beyond the keep count is deleted.
    fn rotate(&self, a: &mut Active) {
        a.file = None; // close before renaming (required on Windows)
        let numbered = |n: u32| {
            let mut name = self.path.clone().into_os_string();
            name.push(format!(".{n}"));
            PathBuf::from(name)
        };
        let keep = self.policy.keep_rotated.max(1);
        let _ = fs::remove_file(numbered(keep));
        for n in (1..keep).rev() {
            let _ = fs::rename(numbered(n), numbered(n + 1));
        }
        let _ = fs::rename(&self.path, numbered(1));
        a.file = open_append(&self.path);
        a.size = 0;
    }
}

fn open_append(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Replace line breaks so one event is always one line.
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

/// What [`prune_logs`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// Files deleted, oldest first.
    pub removed: Vec<PathBuf>,
    /// Bytes freed.
    pub freed_bytes: u64,
}

/// A closed, rotated log: `<name>.log.<number>`.
fn is_rotated(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, n)| {
        !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && Path::new(stem)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("log"))
    })
}

/// Apply the retention rule to `dir` as of `now`.
///
/// Only rotated files (`*.log.N`) are ever deleted. Errors on individual
/// files (in use, no permission) are skipped; the next run tries again.
#[must_use]
pub fn prune_logs(dir: &Path, policy: &LogPolicy, now: SystemTime) -> PruneReport {
    struct Entry {
        path: PathBuf,
        size: u64,
        modified: SystemTime,
        rotated: bool,
    }
    let mut entries: Vec<Entry> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| Entry {
                rotated: is_rotated(&e.file_name().to_string_lossy()),
                size: meta.len(),
                modified: meta.modified().unwrap_or(now),
                path: e.path(),
            })
        })
        .collect();
    entries.sort_by_key(|e| e.modified);

    let mut report = PruneReport::default();
    let mut total: u64 = entries.iter().map(|e| e.size).sum();
    let mut remove = |e: &Entry, total: &mut u64| {
        if fs::remove_file(&e.path).is_ok() {
            *total = total.saturating_sub(e.size);
            report.freed_bytes += e.size;
            report.removed.push(e.path.clone());
            true
        } else {
            false
        }
    };
    // Rule 1: too old.
    let mut kept: Vec<&Entry> = Vec::new();
    for e in &entries {
        let age = now.duration_since(e.modified).unwrap_or_default();
        if e.rotated && age > policy.max_age {
            if !remove(e, &mut total) {
                kept.push(e);
            }
        } else {
            kept.push(e);
        }
    }
    // Rule 2: folder too big, oldest rotated first.
    for e in kept.into_iter().filter(|e| e.rotated) {
        if total <= policy.max_total_bytes {
            break;
        }
        remove(e, &mut total);
    }
    report
}

/// The last `max_lines` lines of the text file at `path` (reads at most the
/// final 256 KiB). Missing files give an empty list.
#[must_use]
pub fn tail_lines(path: &Path, max_lines: usize) -> Vec<String> {
    const WINDOW: u64 = 256 * 1024;
    let Ok(mut f) = File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(WINDOW);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if f.take(WINDOW).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // probably cut mid-line
    }
    let from = lines.len().saturating_sub(max_lines);
    lines[from..].iter().map(|l| (*l).to_string()).collect()
}

/// `YYYY-MM-DDTHH:MM:SSZ` for now, from the system clock.
pub(crate) fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339_utc(secs)
}

/// Format Unix seconds as UTC RFC 3339 (civil-from-days, no time-zone data).
pub(crate) fn rfc3339_utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cl-host-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_aged(path: &Path, bytes: usize, age_days: u64, now: SystemTime) {
        fs::write(path, vec![b'x'; bytes]).unwrap();
        let f = OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(now - Duration::from_hours(age_days * 24))
            .unwrap();
    }

    fn numbered_lines(prefix: &str, range: std::ops::RangeInclusive<u32>, width: usize) -> String {
        use std::fmt::Write as _;
        range.fold(String::new(), |mut s, i| {
            let _ = writeln!(s, "{prefix}{i:0width$}");
            s
        })
    }

    #[test]
    fn timestamps_are_correct() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(1_791_597_789), "2026-10-10T02:03:09Z");
    }

    #[test]
    fn lines_are_timestamped_single_lines() {
        let dir = scratch("lines");
        let log = HostLog::open(dir.join("host.log"));
        log.info("first\nsecond");
        log.warn("careful");
        let text = fs::read_to_string(log.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("INFO first second"), "{}", lines[0]);
        assert!(lines[1].contains("WARN careful"));
        assert!(lines[0].starts_with("20"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unwritable_log_does_not_stop_the_host() {
        let log = HostLog::open(
            std::env::temp_dir()
                .join("cl-host-no-such-dir-xyz")
                .join("a.log"),
        );
        log.error("still fine");
    }

    #[test]
    fn the_log_rotates_by_size_and_keeps_a_fixed_number() {
        let dir = scratch("rotate");
        let policy = LogPolicy {
            rotate_bytes: 200,
            keep_rotated: 2,
            ..LogPolicy::default()
        };
        let log = HostLog::open_with(dir.join("host.log"), policy);
        for i in 0..60 {
            log.info(&format!("event number {i} with some padding to take space"));
        }
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"host.log".to_string()), "{names:?}");
        assert!(names.contains(&"host.log.1".to_string()), "{names:?}");
        assert!(names.contains(&"host.log.2".to_string()), "{names:?}");
        assert!(!names.contains(&"host.log.3".to_string()), "{names:?}");
        // The newest event is in the active file, nothing is lost mid-line.
        let active = fs::read_to_string(dir.join("host.log")).unwrap();
        assert!(active.contains("event number 59"));
        for n in names {
            let size = fs::metadata(dir.join(&n)).unwrap().len();
            assert!(size < 400, "{n} is {size} bytes");
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rotated_names_are_recognised() {
        assert!(is_rotated("engine.log.1"));
        assert!(is_rotated("host.log.12"));
        assert!(!is_rotated("engine.log"));
        assert!(!is_rotated("notes.txt.1"));
        assert!(!is_rotated("host.log.old"));
        assert!(!is_rotated("host.log."));
    }

    #[test]
    fn files_older_than_thirty_days_are_pruned_but_active_logs_never() {
        let dir = scratch("age");
        let now = SystemTime::now();
        write_aged(&dir.join("host.log"), 10, 90, now); // active: even if old
        write_aged(&dir.join("engine.log"), 10, 90, now);
        write_aged(&dir.join("host.log.1"), 10, 31, now);
        write_aged(&dir.join("engine.log.2"), 10, 45, now);
        write_aged(&dir.join("engine.log.1"), 10, 29, now);
        write_aged(&dir.join("readme.txt"), 10, 99, now);
        let report = prune_logs(&dir, &LogPolicy::default(), now);
        assert_eq!(report.removed.len(), 2, "{:?}", report.removed);
        assert_eq!(report.freed_bytes, 20);
        for keep in ["host.log", "engine.log", "engine.log.1", "readme.txt"] {
            assert!(dir.join(keep).exists(), "{keep} should stay");
        }
        for gone in ["host.log.1", "engine.log.2"] {
            assert!(!dir.join(gone).exists(), "{gone} should go");
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_folder_over_the_size_limit_loses_its_oldest_rotated_files_first() {
        let dir = scratch("size");
        let now = SystemTime::now();
        let policy = LogPolicy {
            max_total_bytes: 250,
            ..LogPolicy::default()
        };
        write_aged(&dir.join("engine.log"), 100, 0, now); // active
        write_aged(&dir.join("engine.log.1"), 100, 1, now);
        write_aged(&dir.join("engine.log.2"), 100, 5, now);
        write_aged(&dir.join("engine.log.3"), 100, 9, now);
        let report = prune_logs(&dir, &policy, now);
        // 400 -> 300 -> 200: the two oldest go, then it fits.
        assert_eq!(report.removed.len(), 2, "{:?}", report.removed);
        assert!(dir.join("engine.log").exists());
        assert!(dir.join("engine.log.1").exists());
        assert!(!dir.join("engine.log.2").exists());
        assert!(!dir.join("engine.log.3").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn active_logs_are_never_deleted_even_when_they_alone_exceed_the_limit() {
        let dir = scratch("active");
        let now = SystemTime::now();
        let policy = LogPolicy {
            max_total_bytes: 10,
            ..LogPolicy::default()
        };
        write_aged(&dir.join("host.log"), 100, 0, now);
        let report = prune_logs(&dir, &policy, now);
        assert_eq!(report, PruneReport::default());
        assert!(dir.join("host.log").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_rotated_file_that_is_still_open_can_be_deleted() {
        // Windows would refuse this if the file had been opened without
        // FILE_SHARE_DELETE; the standard library opens with it.
        let dir = scratch("open");
        let now = SystemTime::now();
        let path = dir.join("engine.log.1");
        write_aged(&path, 10, 40, now);
        let held = File::open(&path).unwrap();
        let report = prune_logs(&dir, &LogPolicy::default(), now);
        assert_eq!(report.removed, vec![path.clone()]);
        assert!(!path.exists());
        drop(held);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rotation_works_while_another_handle_is_open() {
        let dir = scratch("rot-open");
        let policy = LogPolicy {
            rotate_bytes: 50,
            keep_rotated: 2,
            ..LogPolicy::default()
        };
        let log = HostLog::open_with(dir.join("host.log"), policy);
        log.info("first line that is long enough to rotate soon");
        let _viewer = File::open(dir.join("host.log")).unwrap(); // a log viewer
        log.info("second line that is long enough to rotate soon");
        log.info("third line that is long enough to rotate soon");
        assert!(dir.join("host.log.1").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_tail_returns_the_last_lines() {
        let dir = scratch("tail");
        let p = dir.join("a.log");
        fs::write(&p, numbered_lines("line ", 1..=500, 0)).unwrap();
        assert_eq!(tail_lines(&p, 3), ["line 498", "line 499", "line 500"]);
        assert_eq!(tail_lines(&p, 1000).len(), 500);
        assert_eq!(
            tail_lines(&dir.join("missing.log"), 5),
            Vec::<String>::new()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_tail_of_a_huge_file_drops_the_cut_first_line() {
        let dir = scratch("tail-big");
        let p = dir.join("big.log");
        let text = numbered_lines("entry ", 0..=59_999, 6);
        fs::write(&p, text).unwrap();
        let lines = tail_lines(&p, 100_000);
        assert!(lines.len() < 60_000);
        assert!(
            lines
                .iter()
                .all(|l| l.starts_with("entry ") && l.len() == 12),
            "cut line kept"
        );
        assert_eq!(lines.last().unwrap(), "entry 059999");
        let _ = fs::remove_dir_all(dir);
    }
}
