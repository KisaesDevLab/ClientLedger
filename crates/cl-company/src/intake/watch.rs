//! Watching one intake folder (A01 plan D3): one thread per confirmed
//! folder, polling (no watcher crate passes the licence rule, and polling
//! behaves the same on SMB and cloud folders).
//!
//! - Looks every [`POLL`] (10 s), and again after [`SETTLE_RECHECK`] (5 s)
//!   while a file is settling, so a file saved at `t` is first seen by
//!   `t + 10 s` and ready by `t + 15 s`.
//! - A look that takes over [`SLOW_SCAN`] doubles the interval (up to a
//!   minute) and shows [`Health::Slow`].
//! - A failure is shown only after two in a row (sync pauses flap); the
//!   retry backs off 10 → 20 → 40 → 60 s.
//! - The folder's thread may hang in the system for a minute on a dead
//!   share. The handle notices on the caller's side: once a call has been
//!   running for [`SCAN_DEADLINE`] the snapshot says
//!   [`Health::NotResponding`]. Dropping the handle sets a stop flag and
//!   returns at once; the thread is **never joined** and exits when its
//!   call returns, its late results going nowhere.
//! - Every [`REPIN_EVERY`] the root is resolved again and compared with
//!   its pin; a changed pin pauses the folder ([`Health::Moved`]): no
//!   reads, no moves, until it matches again.
//! - Reads ([`WatchHandle::read`]) and moves ([`WatchHandle::push_move`])
//!   run on the folder's thread, one at a time, so the caller never waits
//!   on the disk and at most one 10 MB buffer is in flight per folder.

use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cl_core::ids::IntakeFolderId;
use cl_settings::inbox_state::FileIdentity;

use super::fs::{FolderFs, RealFs};
use super::handled::HandledDir;
use super::path::canonical_key_with;
use super::scan::{Listing, STABLE_FOR, Stability, scan};
use super::{FileError, Health, ReadFile};

/// How often a folder is looked at.
pub const POLL: Duration = Duration::from_secs(10);
/// How soon a folder is looked at again while a file settles.
pub const SETTLE_RECHECK: Duration = Duration::from_secs(5);
/// How long one call on a folder may run before it is "not responding".
pub const SCAN_DEADLINE: Duration = Duration::from_secs(15);
/// A look slower than this doubles the interval.
pub const SLOW_SCAN: Duration = Duration::from_secs(5);
/// The longest interval (slow folders, back-off).
pub const MAX_INTERVAL: Duration = Duration::from_secs(60);
/// How often the root's pin is checked.
pub const REPIN_EVERY: Duration = Duration::from_secs(60);

/// The watcher's timings (the plan's by default; tests shorten them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchConfig {
    /// [`POLL`].
    pub poll: Duration,
    /// [`SETTLE_RECHECK`].
    pub settle_recheck: Duration,
    /// [`STABLE_FOR`].
    pub stable_for: Duration,
    /// [`SCAN_DEADLINE`].
    pub deadline: Duration,
    /// [`SLOW_SCAN`].
    pub slow: Duration,
    /// [`MAX_INTERVAL`].
    pub max_interval: Duration,
    /// [`REPIN_EVERY`].
    pub repin_every: Duration,
    /// The first retry after a failure (then doubled).
    pub backoff: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        WatchConfig {
            poll: POLL,
            settle_recheck: SETTLE_RECHECK,
            stable_for: STABLE_FOR,
            deadline: SCAN_DEADLINE,
            slow: SLOW_SCAN,
            max_interval: MAX_INTERVAL,
            repin_every: REPIN_EVERY,
            backoff: POLL,
        }
    }
}

impl WatchConfig {
    /// The wait before the next look: soon while something settles,
    /// otherwise the folder's interval.
    #[must_use]
    pub fn next_wait(&self, settling: bool, interval: Duration) -> Duration {
        if settling {
            self.settle_recheck.min(interval)
        } else {
            interval
        }
    }

    /// The retry wait after `failures` failures in a row (≥ 1).
    #[must_use]
    pub fn backoff_after(&self, failures: u32) -> Duration {
        let factor = 1u32 << failures.saturating_sub(1).min(16);
        self.backoff.saturating_mul(factor).min(self.max_interval)
    }
}

/// One look at the folder, as the caller sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The folder.
    pub folder: IntakeFolderId,
    /// What was found, or why nothing could be.
    pub listing: Result<Listing, Health>,
    /// Counts up with every snapshot.
    pub seq: u64,
    /// How the folder is doing (`Ok`, `Slow`, `TooMany` and `CantMove`
    /// with a listing; the listing's error otherwise).
    pub health: Health,
}

/// A handled original to move.
#[derive(Clone, PartialEq, Eq)]
pub struct MoveJob {
    /// The file (directly in the folder).
    pub path: PathBuf,
    /// The SHA-256 of the bytes copied in: a changed file is not moved.
    pub sha256: String,
    /// Where it goes.
    pub to: HandledDir,
    /// Which inbox entry it is (handed back in [`MoveDone`]).
    pub file: FileIdentity,
}

impl fmt::Debug for MoveJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MoveJob")
            .field("to", &self.to)
            .finish_non_exhaustive()
    }
}

/// A move that ran.
#[derive(Clone, PartialEq, Eq)]
pub struct MoveDone {
    /// Which inbox entry it was.
    pub file: FileIdentity,
    /// Where it was to go.
    pub to: HandledDir,
    /// Where the file is now, or why it didn't move (the original stays).
    pub result: Result<PathBuf, FileError>,
    /// The system refused for lack of permission (the folder shows
    /// [`Health::CantMove`] until a move works).
    pub denied: bool,
}

impl fmt::Debug for MoveDone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MoveDone")
            .field("to", &self.to)
            .field("ok", &self.result.is_ok())
            .field("error", &self.result.as_ref().err())
            .field("denied", &self.denied)
            .finish_non_exhaustive()
    }
}

type ReadReply = Sender<Result<ReadFile, FileError>>;

#[derive(Default)]
struct Inner {
    stop: bool,
    poke: bool,
    seq: u64,
    latest: Option<Snapshot>,
    /// When the thread's current call on the folder started.
    busy_since: Option<Instant>,
    /// "Not responding" was published for the current call.
    stuck_reported: bool,
    reads: VecDeque<(PathBuf, ReadReply)>,
    moves: VecDeque<MoveJob>,
    moved: Vec<MoveDone>,
}

struct Shared {
    id: IntakeFolderId,
    deadline: Duration,
    inner: Mutex<Inner>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn publish(&self, listing: Result<Listing, Health>, health: Health) {
        let mut i = self.lock();
        i.seq += 1;
        let seq = i.seq;
        i.stuck_reported = false;
        i.latest = Some(Snapshot {
            folder: self.id,
            listing,
            seq,
            health,
        });
    }

    /// Runs one blocking call on the folder, marked busy so a hang shows.
    fn busy<T>(&self, f: impl FnOnce() -> T) -> T {
        self.lock().busy_since = Some(Instant::now());
        let out = f();
        self.lock().busy_since = None;
        out
    }
}

/// One folder's watcher. Dropping it stops the thread (never joined).
pub struct WatchHandle {
    shared: Arc<Shared>,
}

impl fmt::Debug for WatchHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchHandle")
            .field("folder", &self.shared.id)
            .finish_non_exhaustive()
    }
}

impl WatchHandle {
    /// Starts watching the confirmed folder `root` whose pin is `pin`
    /// ([`super::canonical_key`] at confirm time).
    #[must_use]
    pub fn spawn(id: IntakeFolderId, root: PathBuf, pin: String) -> WatchHandle {
        WatchHandle::spawn_with(id, root, pin, Arc::new(RealFs), WatchConfig::default())
    }

    /// [`WatchHandle::spawn`] through `fs` with `config` (tests).
    #[must_use]
    pub fn spawn_with(
        id: IntakeFolderId,
        root: PathBuf,
        pin: String,
        fs: Arc<dyn FolderFs>,
        config: WatchConfig,
    ) -> WatchHandle {
        let shared = Arc::new(Shared {
            id,
            deadline: config.deadline,
            inner: Mutex::new(Inner::default()),
            wake: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        let started = std::thread::Builder::new()
            .name("cl-intake-folder".into())
            .spawn(move || {
                Watcher::new(thread_shared, fs, root, pin, config).run();
            });
        if started.is_err() {
            tracing::warn!("could not start an intake folder thread");
            shared.publish(Err(Health::NotResponding), Health::NotResponding);
        }
        WatchHandle { shared }
    }

    /// The folder watched.
    #[must_use]
    pub fn id(&self) -> IntakeFolderId {
        self.shared.id
    }

    /// The latest snapshot, when it is newer than `seq`. A call that has
    /// been running past the deadline shows as a `NotResponding` snapshot.
    #[must_use]
    pub fn latest_after(&self, seq: Option<u64>) -> Option<Snapshot> {
        let mut i = self.shared.lock();
        if !i.stuck_reported
            && i.busy_since
                .is_some_and(|since| since.elapsed() >= self.shared.deadline)
        {
            i.seq += 1;
            i.stuck_reported = true;
            i.latest = Some(Snapshot {
                folder: self.shared.id,
                listing: Err(Health::NotResponding),
                seq: i.seq,
                health: Health::NotResponding,
            });
        }
        i.latest.as_ref().filter(|s| Some(s.seq) != seq).cloned()
    }

    /// Look at the folder now.
    pub fn poke(&self) {
        self.shared.lock().poke = true;
        self.shared.wake.notify_all();
    }

    /// Reads `path` (a file directly in the folder) on the folder's thread
    /// for upload; poll the receiver with `try_recv`. The sender is dropped
    /// unanswered when the handle is.
    #[must_use]
    pub fn read(&self, path: PathBuf) -> Receiver<Result<ReadFile, FileError>> {
        let (tx, rx) = mpsc::channel();
        self.shared.lock().reads.push_back((path, tx));
        self.shared.wake.notify_all();
        rx
    }

    /// Queues a move of a handled original.
    pub fn push_move(&self, job: MoveJob) {
        self.shared.lock().moves.push_back(job);
        self.shared.wake.notify_all();
    }

    /// The moves that ran since the last call.
    #[must_use]
    pub fn take_moved(&self) -> Vec<MoveDone> {
        std::mem::take(&mut self.shared.lock().moved)
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        let mut i = self.shared.lock();
        i.stop = true;
        // Unanswered reads fail at once (their senders go).
        i.reads.clear();
        drop(i);
        self.shared.wake.notify_all();
    }
}

struct Watcher {
    shared: Arc<Shared>,
    fs: Arc<dyn FolderFs>,
    root: PathBuf,
    pin: String,
    config: WatchConfig,
    stability: Stability,
    failures: u32,
    interval: Duration,
    repinned: Option<Instant>,
    /// The pin no longer matches: nothing is read, moved or looked at.
    paused: bool,
    /// The last move was refused for lack of permission.
    cant_move: bool,
}

/// What a wake-up brought.
struct Work {
    reads: Vec<(PathBuf, ReadReply)>,
    moves: Vec<MoveJob>,
}

impl Watcher {
    fn new(
        shared: Arc<Shared>,
        fs: Arc<dyn FolderFs>,
        root: PathBuf,
        pin: String,
        config: WatchConfig,
    ) -> Watcher {
        Watcher {
            shared,
            fs,
            root,
            pin,
            stability: Stability::with_stable_for(config.stable_for),
            failures: 0,
            interval: config.poll,
            repinned: None,
            paused: false,
            cant_move: false,
            config,
        }
    }

    fn stopped(&self) -> bool {
        self.shared.lock().stop
    }

    fn run(mut self) {
        let mut wait = None;
        loop {
            let Some(work) = self.wait_for_work(wait) else {
                return;
            };
            self.check_pin();
            if self.stopped() {
                return;
            }
            if self.paused {
                for (_, reply) in work.reads {
                    let _ = reply.send(Err(FileError::Io));
                }
                // Moves stay queued; nothing is looked at.
                let mut i = self.shared.lock();
                for m in work.moves.into_iter().rev() {
                    i.moves.push_front(m);
                }
                drop(i);
                wait = Some(self.config.repin_every.min(self.config.max_interval));
                continue;
            }
            if !self.do_work(work) {
                return;
            }
            wait = self.look();
            if wait.is_none() {
                return;
            }
        }
    }

    /// Waits `wait` (none: not at all) or until poked, a read comes, or
    /// (unless paused) a move; `None` once stopped.
    fn wait_for_work(&self, wait: Option<Duration>) -> Option<Work> {
        let mut i = self.shared.lock();
        if let Some(wait) = wait {
            let paused = self.paused;
            i = self
                .shared
                .wake
                .wait_timeout_while(i, wait, |i| {
                    !i.stop && !i.poke && i.reads.is_empty() && (paused || i.moves.is_empty())
                })
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        if i.stop {
            return None;
        }
        i.poke = false;
        Some(Work {
            reads: i.reads.drain(..).collect(),
            moves: if self.paused {
                Vec::new()
            } else {
                i.moves.drain(..).collect()
            },
        })
    }

    /// Resolves the root again when due; a different pin pauses the
    /// folder, the right one resumes it.
    fn check_pin(&mut self) {
        if self
            .repinned
            .is_some_and(|t| t.elapsed() < self.config.repin_every)
        {
            return;
        }
        match self
            .shared
            .busy(|| canonical_key_with(&*self.fs, &self.root))
        {
            Ok(key) => {
                self.repinned = Some(Instant::now());
                let elsewhere = key != self.pin;
                if elsewhere != self.paused {
                    self.paused = elsewhere;
                    if elsewhere {
                        self.stability.clear();
                        tracing::warn!("an intake folder now points elsewhere; paused");
                        self.shared.publish(Err(Health::Moved), Health::Moved);
                    }
                }
            }
            // Unreachable or missing: the look says so; the pin is checked
            // again next time.
            Err(_) => self.repinned = None,
        }
    }

    /// Runs the queued reads and moves; false once stopped.
    fn do_work(&mut self, work: Work) -> bool {
        for (path, reply) in work.reads {
            let result = if path.parent() == Some(self.root.as_path()) {
                self.shared.busy(|| self.fs.read(&path))
            } else {
                Err(FileError::NotAFile)
            };
            let _ = reply.send(result);
            if self.stopped() {
                return false;
            }
        }
        for m in work.moves {
            let result = self
                .shared
                .busy(|| self.fs.move_handled(&self.root, &m.path, &m.sha256, m.to));
            let denied = result.as_ref().err().is_some_and(|f| f.denied);
            if result.is_ok() {
                self.cant_move = false;
            } else {
                self.cant_move |= denied;
                tracing::warn!("an intake folder file could not be moved; it stays in place");
            }
            self.shared.lock().moved.push(MoveDone {
                file: m.file,
                to: m.to,
                result: result.map_err(|f| f.error),
                denied,
            });
            if self.stopped() {
                return false;
            }
        }
        true
    }

    /// Looks at the folder and publishes what it found; the wait before
    /// the next look, `None` once stopped.
    fn look(&mut self) -> Option<Duration> {
        let started = Instant::now();
        let fs = Arc::clone(&self.fs);
        let root = self.root.clone();
        let stability = &mut self.stability;
        let listing = self
            .shared
            .busy(|| scan(&*fs, &root, stability, Instant::now()));
        let took = started.elapsed();
        let was_stuck = self.shared.lock().stuck_reported;
        if self.stopped() {
            return None;
        }
        match listing {
            Ok(l) => {
                self.failures = 0;
                let slow = took > self.config.slow;
                self.interval = if slow {
                    self.interval
                        .saturating_mul(2)
                        .min(self.config.max_interval)
                } else {
                    self.config.poll
                };
                let health = if self.cant_move {
                    Health::CantMove
                } else if l.truncated {
                    Health::TooMany
                } else if slow {
                    Health::Slow
                } else {
                    Health::Ok
                };
                let wait = self.config.next_wait(l.settling(), self.interval);
                self.shared.publish(Ok(l), health);
                Some(wait)
            }
            Err(h) => {
                self.failures = self.failures.saturating_add(1);
                self.stability.clear();
                // One failure isn't shown (sync pauses flap), unless it
                // ends a "not responding".
                if self.failures >= 2 || was_stuck {
                    self.shared.publish(Err(h), h);
                }
                Some(self.config.backoff_after(self.failures))
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::super::fs::FakeFs;
    use super::super::scan::ItemState;
    use super::super::sha256_hex;
    use super::*;
    use std::path::Path;

    const PDF: &[u8] = b"%PDF-1.4\n%watch test\n";

    fn id(n: u8) -> IntakeFolderId {
        IntakeFolderId::parse(&format!("7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a{n:02x}")).expect("id")
    }

    fn fast() -> WatchConfig {
        WatchConfig {
            poll: Duration::from_millis(40),
            settle_recheck: Duration::from_millis(20),
            stable_for: Duration::from_millis(60),
            deadline: Duration::from_secs(1),
            slow: Duration::from_millis(150),
            max_interval: Duration::from_millis(400),
            repin_every: Duration::from_millis(100),
            backoff: Duration::from_millis(30),
        }
    }

    fn identity(n: u8) -> FileIdentity {
        FileIdentity {
            path_hash: format!("p{n}"),
            size: 1,
            modified_ms: 1,
            sha256: sha256_hex(PDF),
        }
    }

    fn pin_of(fs: &FakeFs, root: &Path) -> String {
        canonical_key_with(fs, root).expect("pin")
    }

    /// Waits up to `within` for a snapshot matching `want`.
    fn wait_for(
        h: &WatchHandle,
        within: Duration,
        want: impl Fn(&Snapshot) -> bool,
    ) -> Option<Snapshot> {
        let until = Instant::now() + within;
        let mut seen = None;
        while Instant::now() < until {
            if let Some(s) = h.latest_after(seen) {
                seen = Some(s.seq);
                if want(&s) {
                    return Some(s);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    fn ready(s: &Snapshot, name: &str) -> bool {
        s.listing.as_ref().is_ok_and(|l| {
            l.files
                .iter()
                .any(|f| f.name == name && f.state == ItemState::Ready)
        })
    }

    fn setup(root: &str) -> (Arc<FakeFs>, PathBuf, String) {
        let fs = Arc::new(FakeFs::new());
        let root = PathBuf::from(root);
        fs.add_dir(&root);
        let pin = pin_of(&fs, &root);
        (fs, root, pin)
    }

    #[test]
    fn a_saved_file_becomes_ready_reads_and_moves() {
        let (fs, root, pin) = setup("/intake/bills");
        let h = WatchHandle::spawn_with(id(1), root.clone(), pin, fs.clone(), fast());
        let s = wait_for(&h, Duration::from_secs(2), |s| s.listing.is_ok()).expect("listed");
        assert_eq!((s.folder, s.health), (id(1), Health::Ok));
        fs.put(&root, "bill.pdf", PDF, 1);
        let s = wait_for(&h, Duration::from_secs(3), |s| ready(s, "bill.pdf")).expect("ready");
        assert_eq!(s.health, Health::Ok);

        let got = h
            .read(root.join("bill.pdf"))
            .recv_timeout(Duration::from_secs(2))
            .expect("answered");
        assert_eq!(got.map(|r| r.bytes), Ok(PDF.to_vec()));
        let outside = h
            .read(PathBuf::from("/elsewhere/x.pdf"))
            .recv_timeout(Duration::from_secs(2))
            .expect("answered");
        assert_eq!(outside, Err(FileError::NotAFile));

        let nov = HandledDir::Processed {
            year: 2026,
            month: 11,
        };
        h.push_move(MoveJob {
            path: root.join("bill.pdf"),
            sha256: sha256_hex(PDF),
            to: nov,
            file: identity(1),
        });
        let until = Instant::now() + Duration::from_secs(2);
        let mut done = Vec::new();
        while done.is_empty() && Instant::now() < until {
            done = h.take_moved();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].file, identity(1));
        assert_eq!(done[0].result, Ok(nov.dir_in(&root).join("bill.pdf")));
        assert!(!format!("{:?}", done[0]).contains("bill"));
        assert_eq!(fs.names(&root), Vec::<String>::new());
    }

    #[test]
    fn a_half_written_file_waits() {
        let (fs, root, pin) = setup("/intake/scans");
        let h = WatchHandle::spawn_with(id(2), root.clone(), pin, fs.clone(), fast());
        let path = root.join("scan.pdf");
        fs.put(&root, "scan.pdf", &PDF[..6], 1);
        fs.set_locked(&path, true);
        assert!(
            wait_for(&h, Duration::from_millis(400), |s| ready(s, "scan.pdf")).is_none(),
            "held open: never ready"
        );
        fs.put(&root, "scan.pdf", PDF, 2);
        fs.set_locked(&path, false);
        assert!(wait_for(&h, Duration::from_secs(3), |s| ready(s, "scan.pdf")).is_some());
    }

    #[test]
    fn an_unreachable_share_is_a_health_and_others_keep_polling() {
        let (fs, root, pin) = setup("/net/scans");
        let other = PathBuf::from("/intake/receipts");
        fs.add_dir(&other);
        let other_pin = pin_of(&fs, &other);
        let h = WatchHandle::spawn_with(id(3), root.clone(), pin, fs.clone(), fast());
        let o = WatchHandle::spawn_with(id(4), other.clone(), other_pin, fs.clone(), fast());
        wait_for(&h, Duration::from_secs(2), |s| s.health == Health::Ok).expect("ok first");
        let code = if cfg!(windows) { 53 } else { 107 };
        // One failure is not shown (sync pauses flap) …
        fs.fail(&root, code, 1);
        h.poke();
        // … two in a row are.
        fs.fail(&root, code, u32::MAX);
        let s = wait_for(&h, Duration::from_secs(3), |s| s.listing.is_err()).expect("shown");
        assert_eq!(
            (s.health, s.listing.clone()),
            (Health::Unreachable, Err(Health::Unreachable))
        );
        // The other folder is still on time.
        fs.put(&other, "r.pdf", PDF, 1);
        assert!(wait_for(&o, Duration::from_secs(3), |s| ready(s, "r.pdf")).is_some());
        fs.heal(&root);
        assert!(wait_for(&h, Duration::from_secs(3), |s| s.health == Health::Ok).is_some());
    }

    #[test]
    fn a_hung_share_is_not_responding_and_drop_does_not_wait() {
        let (fs, root, pin) = setup("/net/hung");
        let h = WatchHandle::spawn_with(id(5), root.clone(), pin, fs.clone(), fast());
        wait_for(&h, Duration::from_secs(2), |s| s.health == Health::Ok).expect("ok first");
        fs.delay(&root, Duration::from_secs(3));
        h.poke();
        let started = Instant::now();
        let s = wait_for(&h, Duration::from_secs(2), |s| {
            s.health == Health::NotResponding
        })
        .expect("not responding");
        assert_eq!(s.listing, Err(Health::NotResponding));
        assert!(started.elapsed() < Duration::from_secs(2));
        // A read queued behind the hung call doesn't block the caller.
        let rx = h.read(root.join("x.pdf"));
        assert!(rx.try_recv().is_err());
        let dropped = Instant::now();
        drop(h);
        assert!(
            dropped.elapsed() < Duration::from_millis(100),
            "never joined"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "unanswered"
        );
    }

    #[test]
    fn a_moved_root_pauses_until_it_matches_again() {
        let (fs, root, pin) = setup("/Users/o/Dropbox");
        fs.put(&root, "a.pdf", PDF, 1);
        let h = WatchHandle::spawn_with(id(6), root.clone(), pin, fs.clone(), fast());
        wait_for(&h, Duration::from_secs(2), |s| s.health == Health::Ok).expect("ok");
        fs.set_canonical(&root, Path::new("/Volumes/Other"));
        let s = wait_for(&h, Duration::from_secs(2), |s| s.health == Health::Moved).expect("moved");
        assert_eq!(s.listing, Err(Health::Moved));
        let r = h
            .read(root.join("a.pdf"))
            .recv_timeout(Duration::from_secs(2))
            .expect("answered");
        assert_eq!(r, Err(FileError::Io), "no reads while paused");
        h.push_move(MoveJob {
            path: root.join("a.pdf"),
            sha256: sha256_hex(PDF),
            to: HandledDir::Discarded,
            file: identity(6),
        });
        std::thread::sleep(Duration::from_millis(250));
        assert!(h.take_moved().is_empty(), "no moves while paused");
        assert_eq!(fs.names(&root), ["a.pdf"]);
        fs.set_canonical(&root, &root);
        assert!(wait_for(&h, Duration::from_secs(2), |s| s.health == Health::Ok).is_some());
        let until = Instant::now() + Duration::from_secs(2);
        let mut done = Vec::new();
        while done.is_empty() && Instant::now() < until {
            done = h.take_moved();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(done.len(), 1, "the queued move ran once resumed");
    }

    #[test]
    fn a_refused_move_shows_cant_move_and_keeps_the_file() {
        let (fs, root, pin) = setup("/net/readonly");
        fs.put(&root, "a.pdf", PDF, 1);
        fs.set_read_only(&root, true);
        let h = WatchHandle::spawn_with(id(7), root.clone(), pin, fs.clone(), fast());
        h.push_move(MoveJob {
            path: root.join("a.pdf"),
            sha256: sha256_hex(PDF),
            to: HandledDir::Discarded,
            file: identity(7),
        });
        let s = wait_for(&h, Duration::from_secs(2), |s| s.health == Health::CantMove)
            .expect("cant move");
        assert!(s.listing.is_ok(), "still listed");
        let done = h.take_moved();
        assert_eq!(done.len(), 1);
        assert!(done[0].denied);
        assert_eq!(done[0].result, Err(FileError::Io));
        assert_eq!(fs.names(&root), ["a.pdf"], "the original stays");
        // A changed file is refused, not moved.
        fs.set_read_only(&root, false);
        h.push_move(MoveJob {
            path: root.join("a.pdf"),
            sha256: sha256_hex(b"other"),
            to: HandledDir::Discarded,
            file: identity(7),
        });
        std::thread::sleep(Duration::from_millis(200));
        let done = h.take_moved();
        assert_eq!(
            done.first().map(|d| d.result.clone()),
            Some(Err(FileError::Changed))
        );
        assert_eq!(fs.names(&root), ["a.pdf"]);
    }

    #[test]
    fn a_slow_folder_is_looked_at_less_often() {
        let (fs, root, pin) = setup("/net/slow");
        fs.delay(&root, Duration::from_millis(160));
        let h = WatchHandle::spawn_with(id(8), root, pin, fs, fast());
        assert!(wait_for(&h, Duration::from_secs(3), |s| s.health == Health::Slow).is_some());
    }

    #[test]
    fn timings() {
        let c = WatchConfig::default();
        assert_eq!(c.next_wait(true, POLL), SETTLE_RECHECK);
        assert_eq!(c.next_wait(false, POLL), POLL);
        let backoff: Vec<_> = (1..=5).map(|n| c.backoff_after(n).as_secs()).collect();
        assert_eq!(backoff, [10, 20, 40, 60, 60]);
    }

    /// AC1 timing with a fake clock: a file saved at any moment between
    /// two looks is first seen within 10 s and ready within 15 s.
    #[test]
    fn arrival_timing_with_a_fake_clock() {
        let c = WatchConfig::default();
        let root = PathBuf::from("/intake/clock");
        for offset_s in 0..10u64 {
            for written_ms in [0u64, 1, 500, 999] {
                let fs = FakeFs::new();
                fs.add_dir(&root);
                let t0 = Instant::now();
                let written = Duration::from_millis(offset_s * 1000 + written_ms);
                let mut stability = Stability::default();
                let mut at = Duration::ZERO;
                let mut first_seen = None;
                let mut ready_at = None;
                while ready_at.is_none() && at < Duration::from_secs(60) {
                    if at >= written {
                        fs.put(&root, "a.pdf", PDF, 1);
                    }
                    let l = scan(&fs, &root, &mut stability, t0 + at).expect("scan");
                    if let Some(f) = l.files.first() {
                        first_seen.get_or_insert(at);
                        if f.state == ItemState::Ready {
                            ready_at = Some(at);
                        }
                    }
                    at += c.next_wait(l.settling(), POLL);
                }
                let seen = first_seen.expect("seen").saturating_sub(written);
                let ready = ready_at.expect("ready").saturating_sub(written);
                assert!(seen <= POLL, "seen after {seen:?}");
                assert!(ready <= POLL + SETTLE_RECHECK, "ready after {ready:?}");
            }
        }
    }
}
