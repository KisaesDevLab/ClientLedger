//! Testing a folder before it is used (A01 plan D2.2): the Admin's
//! **Test** and **Use this folder** run [`probe_async`], which never blocks
//! the caller and answers within [`PROBE_DEADLINE`] even when the share
//! hangs.
//!
//! The probe reports whether the folder is reachable and a folder, what
//! would come in (and how many files are too large, HEIC, online only or
//! hard-linked), whether files can be moved out of it (a hidden
//! `.clientledger-write-test-…` file is created with `create_new` and
//! removed; no owner file is ever opened for writing), cloud-folder hints,
//! and the root's [`super::canonical_key`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use super::Health;
use super::fs::{FolderFs, RealFs};
use super::path::{classify, key_of};
use super::scan::{ItemState, Stability, scan};

/// How long a probe may take before it reports
/// [`Health::NotResponding`].
pub const PROBE_DEADLINE: Duration = Duration::from_secs(15);

/// What would come in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeCounts {
    /// Files that would be copied in.
    pub incoming: usize,
    /// Over 10 MB.
    pub too_large: usize,
    /// HEIC/HEIF.
    pub not_supported: usize,
    /// Online only (cloud placeholders).
    pub online_only: usize,
    /// Hard-linked (Unix).
    pub linked: usize,
    /// Left out over the listing cap.
    pub more: usize,
}

/// What a probe found.
#[derive(Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// `Ok`, or what is wrong (`Missing`, `Unreachable`, `NotResponding`,
    /// `PermissionDenied`, `NotAFolder`; `TooMany` when the folder has more
    /// entries than one look reads).
    pub health: Health,
    /// What would come in (zero when the folder couldn't be read).
    pub counts: ProbeCounts,
    /// Whether handled files can be moved out (`None`: not checked, e.g.
    /// "Leave originals where they are", or the folder couldn't be read).
    pub writable: Option<bool>,
    /// The folder looks like a cloud-sync folder (`OneDrive`, `Dropbox`,
    /// Google Drive, iCloud, Box), or holds online-only files.
    pub cloud: bool,
    /// The root's pin, stored when the folder is confirmed.
    pub canonical_key: Option<String>,
}

/// Counts and flags only: the pin is a hash of the path, and stays out too.
impl fmt::Debug for ProbeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProbeReport")
            .field("health", &self.health)
            .field("counts", &self.counts)
            .field("writable", &self.writable)
            .field("cloud", &self.cloud)
            .finish_non_exhaustive()
    }
}

impl ProbeReport {
    fn failed(health: Health) -> ProbeReport {
        ProbeReport {
            health,
            counts: ProbeCounts::default(),
            writable: None,
            cloud: false,
            canonical_key: None,
        }
    }

    /// The report when the deadline passed ("Not responding (still
    /// trying)").
    #[must_use]
    pub fn not_responding() -> ProbeReport {
        ProbeReport::failed(Health::NotResponding)
    }

    /// True when the folder can be confirmed (reachable, a folder).
    #[must_use]
    pub fn usable(&self) -> bool {
        matches!(self.health, Health::Ok | Health::TooMany) && self.canonical_key.is_some()
    }
}

/// True when a path part names a cloud-sync client's folder.
fn cloud_hint(canonical: &Path) -> bool {
    // Split on both separators, so a Windows path reads the same on every
    // system.
    let text = canonical.to_string_lossy().to_lowercase();
    text.split(['/', '\\']).any(|part| {
        part.starts_with("onedrive")
            || part.starts_with("dropbox")
            || part.contains("google drive")
            || part == "googledrive"
            || part == "my drive"
            || part.contains("icloud")
            || part == "cloudstorage"
            || part == "mobile documents"
            || part == "box"
            || part == "box sync"
    })
}

/// Probes `root` through `fs`, blocking (run it on a background thread).
#[must_use]
pub fn probe(fs: &dyn FolderFs, root: &Path, check_write: bool) -> ProbeReport {
    let canonical = match fs.canonicalize(root) {
        Ok(c) => c,
        Err(e) => return ProbeReport::failed(classify(&e, true)),
    };
    let listing = match scan(fs, root, &mut Stability::default(), Instant::now()) {
        Ok(l) => l,
        Err(h) => return ProbeReport::failed(h),
    };
    let mut counts = ProbeCounts {
        more: listing.more,
        ..ProbeCounts::default()
    };
    for f in &listing.files {
        match f.state {
            ItemState::Settling | ItemState::Ready => counts.incoming += 1,
            ItemState::TooLarge => counts.too_large += 1,
            ItemState::NotSupported => counts.not_supported += 1,
            ItemState::OnlineOnly => counts.online_only += 1,
            ItemState::Linked => counts.linked += 1,
        }
    }
    let writable = check_write.then(|| fs.write_probe(root).is_ok());
    ProbeReport {
        health: if listing.truncated {
            Health::TooMany
        } else {
            Health::Ok
        },
        counts,
        writable,
        cloud: counts.online_only > 0 || cloud_hint(&canonical),
        canonical_key: Some(key_of(&canonical)),
    }
}

/// [`probe_async`] through `fs` with its own deadline (tests).
#[must_use]
pub fn probe_async_with(
    fs: Arc<dyn FolderFs>,
    root: PathBuf,
    check_write: bool,
    deadline: Duration,
) -> Receiver<ProbeReport> {
    let (out_tx, out_rx) = mpsc::channel();
    let (in_tx, in_rx) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("cl-intake-probe".into())
        .spawn(move || {
            let _ = in_tx.send(probe(&*fs, &root, check_write));
        });
    if worker.is_err() {
        tracing::warn!("could not start an intake folder probe");
        let _ = out_tx.send(ProbeReport::not_responding());
        return out_rx;
    }
    // The waiter forwards the report, or "not responding" once the deadline
    // passed; the worker is abandoned (never joined) and its late report
    // dropped.
    let waiter = std::thread::Builder::new()
        .name("cl-intake-probe-wait".into())
        .spawn({
            let out_tx = out_tx.clone();
            move || {
                let report = in_rx
                    .recv_timeout(deadline)
                    .unwrap_or_else(|_| ProbeReport::not_responding());
                let _ = out_tx.send(report);
            }
        });
    if waiter.is_err() {
        let _ = out_tx.send(ProbeReport::not_responding());
    }
    out_rx
}

/// Probes `root` on a background thread; the receiver gets exactly one
/// report within [`PROBE_DEADLINE`] (poll it with `try_recv` each frame).
/// Run at most one probe per folder at a time: the caller keeps the
/// receiver and starts no other until it answered.
#[must_use]
pub fn probe_async(root: PathBuf, check_write: bool) -> Receiver<ProbeReport> {
    probe_async_with(Arc::new(RealFs), root, check_write, PROBE_DEADLINE)
}

#[cfg(test)]
mod tests {
    use super::super::fs::FakeFs;
    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n";

    #[test]
    fn a_real_folder() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("a.pdf"), PDF).expect("write");
        std::fs::write(dir.path().join("b.heic"), b"x").expect("write");
        std::fs::write(dir.path().join("notes.txt"), b"x").expect("write");
        let r = probe(&RealFs, dir.path(), true);
        assert_eq!(r.health, Health::Ok);
        assert_eq!(r.counts.incoming, 1);
        assert_eq!(r.counts.not_supported, 1);
        assert_eq!(r.writable, Some(true));
        assert!(r.usable());
        assert_eq!(
            r.canonical_key,
            super::super::canonical_key(dir.path()).ok()
        );
        // The write test left nothing behind, and touched no owner file.
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .expect("dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a.pdf", "b.heic", "notes.txt"]);
        assert_eq!(std::fs::read(dir.path().join("a.pdf")).expect("read"), PDF);
        let r = probe(&RealFs, dir.path(), false);
        assert_eq!(r.writable, None, "keepOriginals: not checked");
        let missing = probe(&RealFs, &dir.path().join("nope"), true);
        assert_eq!(missing.health, Health::Missing);
        assert!(!missing.usable());
        let file = probe(&RealFs, &dir.path().join("a.pdf"), true);
        assert_eq!(file.health, Health::NotAFolder);
    }

    #[test]
    fn report_debug_has_no_pin() {
        let dir = tempfile::tempdir().expect("tmp");
        let r = probe(&RealFs, dir.path(), false);
        let key = r.canonical_key.clone().expect("key");
        assert!(!format!("{r:?}").contains(&key));
    }

    #[test]
    fn a_read_only_or_unreachable_share() {
        let fs = FakeFs::new();
        let root = Path::new(r"/net/scans");
        fs.put(root, "a.pdf", PDF, 1);
        fs.put_online_only(root, "b.pdf", 10);
        fs.set_read_only(root, true);
        let r = probe(&fs, root, true);
        assert_eq!(r.health, Health::Ok);
        assert_eq!(r.writable, Some(false));
        assert!(r.cloud, "online-only files say cloud");
        assert_eq!((r.counts.incoming, r.counts.online_only), (1, 1));
        fs.fail(root, if cfg!(windows) { 67 } else { 107 }, u32::MAX);
        assert_eq!(probe(&fs, root, true).health, Health::Unreachable);
    }

    #[test]
    fn cloud_hints() {
        for p in [
            "/Users/o/Library/CloudStorage/Dropbox/Bills",
            "/Users/o/Dropbox/Bills",
            r"C:\Users\o\OneDrive - Smith Plumbing\Bills",
            "/Users/o/Library/Mobile Documents/com~apple~CloudDocs/x",
            r"G:\My Drive\Receipts",
        ] {
            assert!(cloud_hint(Path::new(p)), "{p}");
        }
        assert!(!cloud_hint(Path::new("/home/o/Scans/boxes")));
    }

    #[test]
    fn a_hung_share_answers_not_responding_by_the_deadline() {
        let fs = Arc::new(FakeFs::new());
        let root = PathBuf::from("/net/hung");
        fs.add_dir(&root);
        fs.delay(&root, Duration::from_secs(3));
        let started = Instant::now();
        let rx = probe_async_with(
            Arc::clone(&fs) as Arc<dyn FolderFs>,
            root.clone(),
            true,
            Duration::from_millis(200),
        );
        let r = rx.recv_timeout(Duration::from_secs(2)).expect("answered");
        assert_eq!(r.health, Health::NotResponding);
        assert!(started.elapsed() < Duration::from_secs(2), "not held up");
        // Exactly one report: the late one is dropped.
        assert!(rx.recv_timeout(Duration::from_secs(4)).is_err());

        fs.heal(&root);
        let rx = probe_async_with(fs, root, false, Duration::from_secs(5));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).expect("ok").health,
            Health::Ok
        );
    }

    #[test]
    fn the_real_probe_runs_off_thread() {
        let dir = tempfile::tempdir().expect("tmp");
        let rx = probe_async(dir.path().to_path_buf(), true);
        let r = rx.recv_timeout(PROBE_DEADLINE).expect("answered");
        assert_eq!((r.health, r.writable), (Health::Ok, Some(true)));
    }
}
