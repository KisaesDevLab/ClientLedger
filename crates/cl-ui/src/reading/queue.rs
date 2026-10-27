//! The reading queue's lanes (A04 D1 "Reading pipeline", S10, S13). Frozen
//! by WP-M; WP-I (Read all, auto-read on arrival) and WP-C (region and word
//! re-reads for an open form) put their jobs here, and the one reading
//! worker takes them in this order:
//!
//! - **Interactive** jobs (a region re-read, the word re-read for an open
//!   form, Read document) always run before the next **batch** job (Read
//!   all, auto-read), so a box dragged during Read all answers before the
//!   batch ends (T13).
//! - A batch job for a draft that is open in a form is skipped and stays
//!   queued (D1, S4: a reading must never change what the owner reviews).
//! - "Reading 3 of 12" counts the batch only; Cancel drops the batch's
//!   waiting jobs and keeps the interactive ones.
//!
//! Generic over the job (`T`), so the queue holds no bytes itself: at most
//! [`BYTES_IN_MEMORY`] queued arrivals may hold their file's bytes, the
//! rest download at their turn (S13, up to [`MAX_DOWNLOAD_BYTES`]).

use std::collections::VecDeque;

/// Queued arrivals that may keep their bytes in memory (S13).
pub const BYTES_IN_MEMORY: usize = 3;
/// The largest file a queued job downloads at its turn (S13, 10 MB).
pub const MAX_DOWNLOAD_BYTES: u64 = 10 * 1024 * 1024;

/// Which lane a job waits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    /// The owner is waiting for it (a form is open): runs first.
    Interactive,
    /// Read all and auto-read: runs when no interactive job waits.
    Batch,
}

/// One queued job: its lane, the draft it reads (if any; batch jobs of a
/// draft open in a form are skipped), and the caller's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued<T> {
    /// The lane.
    pub lane: Lane,
    /// The engine draft id it reads, as text (`None`: not a draft yet,
    /// such as an arrival before its claim resolved).
    pub draft: Option<String>,
    /// The caller's job.
    pub job: T,
}

/// How far the batch is: "Reading `done + 1` of `total`".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchProgress {
    /// Batch jobs handed out since the batch began.
    pub done: usize,
    /// Batch jobs in this batch (handed out and waiting).
    pub total: usize,
}

/// The two-lane queue (see the module comment). Plain data; the reading
/// worker's thread or the screen that owns it calls [`ReadQueue::next`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadQueue<T> {
    interactive: VecDeque<Queued<T>>,
    batch: VecDeque<Queued<T>>,
    batch_done: usize,
}

impl<T> Default for ReadQueue<T> {
    fn default() -> Self {
        ReadQueue {
            interactive: VecDeque::new(),
            batch: VecDeque::new(),
            batch_done: 0,
        }
    }
}

impl<T> ReadQueue<T> {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        ReadQueue::default()
    }

    /// Queues `job` at the back of `lane`. A batch job added to an idle
    /// batch starts a new count.
    pub fn push(&mut self, lane: Lane, draft: Option<String>, job: T) {
        let q = Queued { lane, draft, job };
        match lane {
            Lane::Interactive => self.interactive.push_back(q),
            Lane::Batch => {
                if self.batch.is_empty() {
                    self.batch_done = 0;
                }
                self.batch.push_back(q);
            }
        }
    }

    /// The next job to run: the oldest interactive job, else the oldest
    /// batch job whose draft isn't open in a form (`open(draft)` says).
    /// Skipped batch jobs keep their place.
    pub fn next(&mut self, open: impl Fn(&str) -> bool) -> Option<Queued<T>> {
        if let Some(q) = self.interactive.pop_front() {
            return Some(q);
        }
        let at = self
            .batch
            .iter()
            .position(|q| !q.draft.as_deref().is_some_and(&open))?;
        let q = self.batch.remove(at)?;
        self.batch_done += 1;
        Some(q)
    }

    /// The batch's progress; `None` when no batch job waits.
    #[must_use]
    pub fn progress(&self) -> Option<BatchProgress> {
        (!self.batch.is_empty()).then(|| BatchProgress {
            done: self.batch_done,
            total: self.batch_done + self.batch.len(),
        })
    }

    /// Cancel: drops every waiting batch job and returns them (the caller
    /// lets go of their bytes); interactive jobs stay.
    pub fn cancel_batch(&mut self) -> Vec<Queued<T>> {
        self.batch_done = 0;
        self.batch.drain(..).collect()
    }

    /// Drops every waiting job of `draft` (it was entered, discarded or
    /// closed elsewhere) and returns them.
    pub fn forget(&mut self, draft: &str) -> Vec<Queued<T>> {
        let mut gone = Vec::new();
        for lane in [&mut self.interactive, &mut self.batch] {
            let (drop, keep): (VecDeque<_>, VecDeque<_>) = lane
                .drain(..)
                .partition(|q| q.draft.as_deref() == Some(draft));
            *lane = keep;
            gone.extend(drop);
        }
        gone
    }

    /// Jobs waiting in `lane`.
    #[must_use]
    pub fn waiting(&self, lane: Lane) -> usize {
        match lane {
            Lane::Interactive => self.interactive.len(),
            Lane::Batch => self.batch.len(),
        }
    }

    /// True when nothing waits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.interactive.is_empty() && self.batch.is_empty()
    }

    /// Whether the batch job at position `index` (0 = next) may hold its
    /// file's bytes now ([`BYTES_IN_MEMORY`], S13).
    #[must_use]
    pub const fn may_hold_bytes(index: usize) -> bool {
        index < BYTES_IN_MEMORY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_jobs_run_before_the_next_batch_job() {
        let mut q = ReadQueue::new();
        q.push(Lane::Batch, Some("a".into()), 1);
        q.push(Lane::Batch, Some("b".into()), 2);
        q.push(Lane::Batch, Some("c".into()), 3);
        assert_eq!(q.progress(), Some(BatchProgress { done: 0, total: 3 }));
        assert_eq!(q.next(|_| false).map(|j| j.job), Some(1));
        // A box is dragged during Read all: it runs next.
        q.push(Lane::Interactive, Some("x".into()), 9);
        assert_eq!(q.waiting(Lane::Interactive), 1);
        let j = q.next(|_| false).expect("job");
        assert_eq!((j.lane, j.job), (Lane::Interactive, 9));
        assert_eq!(q.progress(), Some(BatchProgress { done: 1, total: 3 }));
        assert_eq!(q.next(|_| false).map(|j| j.job), Some(2));
        assert_eq!(q.next(|_| false).map(|j| j.job), Some(3));
        assert_eq!(q.progress(), None);
        assert!(q.is_empty());
        assert!(q.next(|_| false).is_none());
        // A new batch counts from zero.
        q.push(Lane::Batch, None, 4);
        assert_eq!(q.progress(), Some(BatchProgress { done: 0, total: 1 }));
    }

    #[test]
    fn drafts_open_in_a_form_are_skipped_and_keep_their_place() {
        let mut q = ReadQueue::new();
        q.push(Lane::Batch, Some("open".into()), 1);
        q.push(Lane::Batch, None, 2);
        q.push(Lane::Batch, Some("b".into()), 3);
        let open = |id: &str| id == "open";
        assert_eq!(q.next(open).map(|j| j.job), Some(2));
        assert_eq!(q.next(open).map(|j| j.job), Some(3));
        assert!(q.next(open).is_none(), "only the open draft waits");
        assert_eq!(q.waiting(Lane::Batch), 1);
        // The form closed: it is read now.
        assert_eq!(q.next(|_| false).map(|j| j.job), Some(1));
        // Interactive jobs for an open draft are what the form asked for.
        q.push(Lane::Interactive, Some("open".into()), 5);
        assert_eq!(q.next(open).map(|j| j.job), Some(5));
    }

    #[test]
    fn cancel_and_forget() {
        let mut q = ReadQueue::new();
        q.push(Lane::Batch, Some("a".into()), 1);
        q.push(Lane::Batch, Some("b".into()), 2);
        q.push(Lane::Interactive, Some("a".into()), 3);
        assert_eq!(q.forget("a").len(), 2);
        assert_eq!(q.waiting(Lane::Interactive), 0);
        q.push(Lane::Interactive, Some("c".into()), 4);
        let dropped = q.cancel_batch();
        assert_eq!(dropped.iter().map(|j| j.job).collect::<Vec<_>>(), vec![2]);
        assert_eq!(q.progress(), None);
        assert_eq!(q.waiting(Lane::Interactive), 1, "interactive jobs stay");
        assert!(ReadQueue::<u8>::may_hold_bytes(2));
        assert!(!ReadQueue::<u8>::may_hold_bytes(BYTES_IN_MEMORY));
        assert_eq!(MAX_DOWNLOAD_BYTES, 10_485_760);
    }
}
