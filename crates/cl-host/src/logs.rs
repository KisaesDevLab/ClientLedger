//! The host's own log file.
//!
//! One line per event: `2026-10-15T14:03:09Z INFO message`. Messages never
//! contain amounts, names, memo text, or secrets (CLAUDE.md logging rule);
//! callers pass text that is already scrubbed.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

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

/// An append-only log file shared between threads.
#[derive(Debug)]
pub struct HostLog {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl HostLog {
    /// Open (creating if needed) the log at `path`. Failure to open is not an
    /// error: the host must run even when its log folder is unwritable, so
    /// lines are then only passed to `tracing`.
    #[must_use]
    pub fn open(path: impl Into<PathBuf>) -> HostLog {
        let path = path.into();
        let file = open_append(&path);
        HostLog {
            path,
            file: Mutex::new(file),
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
        if let Ok(mut guard) = self.file.lock()
            && let Some(f) = guard.as_mut()
        {
            let _ = f.write_all(line.as_bytes());
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
}

fn open_append(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Replace line breaks so one event is always one line.
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
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

    #[test]
    fn timestamps_are_correct() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(1_791_597_789), "2026-10-10T02:03:09Z");
    }

    #[test]
    fn lines_are_timestamped_single_lines() {
        let dir = std::env::temp_dir().join(format!("cl-host-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = HostLog::open(dir.join("host.log"));
        log.info("first\nsecond");
        log.warn("careful");
        let text = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("INFO first second"), "{}", lines[0]);
        assert!(lines[1].contains("WARN careful"));
        assert!(lines[0].starts_with("20"));
        let _ = std::fs::remove_dir_all(dir);
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
}
