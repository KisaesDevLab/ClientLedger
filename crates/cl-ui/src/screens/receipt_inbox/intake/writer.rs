//! Saving the intake inbox state off the egui thread (A01 review S8): the
//! file holds every company's entries (up to a few MB), and saving it fits
//! it under its cap, writes it and syncs it to disk.
//!
//! [`StateWriter::save`] hands a copy to the writer thread and returns at
//! once. Saves coalesce: when several queue up while one is being written,
//! only the latest is written next (each is a whole state). Each save gets
//! a number; [`StateWriter::written`] says which one is on disk, so a step
//! that must be saved before its side effect (an upload, D4) waits for it
//! without blocking the frame. [`StateWriter::flush`] waits (bounded) for
//! everything queued, before anything else reads or writes the file.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cl_settings::inbox_state::{InboxState, InboxStateStore};

/// How long dropping the writer waits for the last save.
const DROP_FLUSH: Duration = Duration::from_secs(3);

#[derive(Default)]
struct Slot {
    /// The latest state not yet taken by the thread, with its number.
    next: Option<(u64, InboxState)>,
    /// The number of the latest save queued.
    queued: u64,
    /// The number of the latest save written (or tried: a failure is
    /// logged and the next save tries again).
    done: u64,
    stop: bool,
}

struct Shared {
    slot: Mutex<Slot>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Saves the inbox state on its own thread.
pub(crate) struct StateWriter {
    shared: Arc<Shared>,
    /// No thread could be started: saves are made in place.
    inline: Option<InboxStateStore>,
}

impl StateWriter {
    /// A writer saving to `store`.
    pub(crate) fn spawn(store: InboxStateStore) -> StateWriter {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot::default()),
            changed: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        let thread_store = store.clone();
        let spawned = std::thread::Builder::new()
            .name("cl-inbox-state".into())
            .spawn(move || run(&thread_shared, &thread_store));
        StateWriter {
            shared,
            inline: spawned.is_err().then_some(store),
        }
    }

    /// Queues `state` to be saved; its number.
    pub(crate) fn save(&self, state: InboxState) -> u64 {
        if let Some(store) = &self.inline {
            save_logged(store, &state);
            let mut s = self.shared.lock();
            s.queued += 1;
            s.done = s.queued;
            return s.done;
        }
        let mut s = self.shared.lock();
        s.queued += 1;
        let n = s.queued;
        s.next = Some((n, state));
        drop(s);
        self.shared.changed.notify_all();
        n
    }

    /// The number of the latest save on disk.
    pub(crate) fn written(&self) -> u64 {
        self.shared.lock().done
    }

    /// Waits up to `timeout` until every queued save is written; true when
    /// it was.
    pub(crate) fn flush(&self, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        let mut s = self.shared.lock();
        while s.done < s.queued {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            s = self
                .shared
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl Drop for StateWriter {
    fn drop(&mut self) {
        // The last save is written before the file is read again (another
        // company, the app closing); bounded, never a hang.
        let _ = self.flush(DROP_FLUSH);
        self.shared.lock().stop = true;
        self.shared.changed.notify_all();
    }
}

fn save_logged(store: &InboxStateStore, state: &InboxState) {
    if let Err(e) = store.save(state) {
        // R6: dedupe then holds for this session only.
        tracing::warn!(error = %e, "could not save the intake inbox state");
    }
}

fn run(shared: &Shared, store: &InboxStateStore) {
    loop {
        let mut s = shared.lock();
        while s.next.is_none() && !s.stop {
            s = shared
                .changed
                .wait(s)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let Some((n, state)) = s.next.take() else {
            return;
        };
        drop(s);
        save_logged(store, &state);
        shared.lock().done = n;
        shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_settings::inbox_state::{FileIdentity, company_key};

    fn state(n: u64) -> InboxState {
        let mut s = InboxState::default();
        let f = FileIdentity {
            path_hash: format!("p{n}"),
            size: n,
            modified_ms: 1,
            sha256: format!("{n:064}"),
        };
        s.mark_duplicate(&company_key("t"), None, &f);
        s
    }

    #[test]
    fn saves_land_in_order_and_the_latest_wins() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(dir.path());
        let w = StateWriter::spawn(store.clone());
        let mut last = 0;
        for n in 1..=50 {
            last = w.save(state(n));
        }
        assert!(w.flush(Duration::from_secs(10)));
        assert_eq!(w.written(), last);
        let back = store.load().expect("load");
        assert_eq!(back.entries(&company_key("t"))[0].file.size, 50);
    }

    #[test]
    fn saving_returns_before_the_file_is_written() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(dir.path());
        let w = StateWriter::spawn(store.clone());
        // A big state: fitting and syncing it takes a while.
        let mut big = InboxState::default();
        let c = company_key("t");
        for n in 0..9000_u64 {
            big.record_seen(
                &c,
                &format!("{n:064}"),
                "d0000000-0000-4000-8000-000000000001",
                1,
            );
        }
        let started = Instant::now();
        let n = w.save(big);
        let took = started.elapsed();
        assert!(took < Duration::from_millis(50), "{took:?}");
        assert!(w.flush(Duration::from_secs(10)));
        assert!(w.written() >= n);
        assert_eq!(store.load().expect("load").seen[&c].len(), 9000);
    }

    #[test]
    fn dropping_writes_what_was_queued() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(dir.path());
        let w = StateWriter::spawn(store.clone());
        w.save(state(7));
        drop(w);
        assert_eq!(
            store.load().expect("load").entries(&company_key("t"))[0]
                .file
                .size,
            7
        );
    }
}
