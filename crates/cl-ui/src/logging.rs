//! Where the UI's own `tracing` events go: a size-rotated `ui.log` next to the
//! host log, written through `cl_host::HostLog` (same rotation and size
//! policy, Q-118). Only events from `cl_ui` modules are taken: the host and
//! supervisor already write their own lines to `host.log`, and filtering by
//! target also keeps `HostLog`'s own `tracing` echo from feeding back into the
//! subscriber. Messages follow the logging rule: no amounts, names or memo
//! text at info level.

use std::io;
use std::path::Path;
use std::sync::Arc;

use cl_host::{HostLog, Level};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, filter};

/// One formatted event; written to the log when dropped.
struct Line {
    buf: Vec<u8>,
    log: Arc<HostLog>,
}

impl io::Write for Line {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.buf);
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let level = if text.starts_with("ERROR") {
            Level::Error
        } else if text.starts_with("WARN") {
            Level::Warn
        } else {
            Level::Info
        };
        self.log.log(level, text);
    }
}

struct Maker(Arc<HostLog>);

impl MakeWriter<'_> for Maker {
    type Writer = Line;

    fn make_writer(&self) -> Line {
        Line {
            buf: Vec::new(),
            log: Arc::clone(&self.0),
        }
    }
}

/// Install the subscriber once per process. Returns false when one is
/// already installed.
pub fn init(path: impl AsRef<Path>) -> bool {
    let log = Arc::new(HostLog::open(path.as_ref()));
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .without_time()
        .with_writer(Maker(log))
        .with_filter(filter::filter_fn(|m| {
            m.target().starts_with("cl_ui") && *m.level() <= tracing::Level::INFO
        }));
    tracing_subscriber::registry()
        .with(layer)
        .try_init()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_events_reach_the_rotating_log_and_nothing_loops() {
        let dir = std::env::temp_dir().join(format!("cl-ui-logging-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ui.log");
        assert!(init(&path));
        assert!(!init(&path), "a second install is refused");
        tracing::warn!("ui-log-marker for the test");
        tracing::debug!("below info: not written");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("ui-log-marker"), "{text}");
        assert!(text.contains("WARN"), "{text}");
        assert!(!text.contains("below info"));
        // Exactly one line: HostLog's own echo is a cl_host event and is filtered out.
        assert_eq!(text.matches("ui-log-marker").count(), 1, "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
