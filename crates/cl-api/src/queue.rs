//! The UI request queue (P05 plan §2.9). No egui here: cl-ui hands in its
//! repaint callback.
//!
//! - The queue owns a multi-thread tokio runtime (2 workers, `cl-api`
//!   threads) off the UI thread. `read`/`write` only spawn and return a
//!   [`Pending`] handle at once; the UI calls [`Pending::try_take`] every
//!   frame and the queue calls `notify` (repaint) when a job finishes.
//! - Backpressure: at most 256 outstanding jobs (`Err(QueueFull)` beyond),
//!   8 concurrent engine requests, and a separate lane of 2 for prefetch.
//! - Cancellation (window closed, [`RequestGroup::cancel`] or drop): reads
//!   are aborted and resolve to `Cancelled`. Writes are **never** aborted
//!   (that would make the outcome unknown): they finish, and their result
//!   goes to [`ApiQueue::take_orphans`]. Cancelling never means "it did not
//!   happen".
//! - Scope (S1): every job runs on a session view pinned to the company
//!   open when it was queued ([`Session::pinned`]). A read that finishes
//!   after a company switch resolves to `StaleScope`. A write never reaches
//!   the next company: an attempt not yet sent fails with `StaleScope`
//!   (nothing sent), and one whose earlier attempt may have reached the
//!   engine is `OutcomeUnknown`. Its result is also copied to the orphans
//!   with the company it was queued (and sent) in.
//! - A write job that panics resolves to `OutcomeUnknown`, never to a
//!   silently closed handle; a read job that panics is `Internal`.
//! - [`ApiQueue::shutdown`] stops intake, cancels reads, waits for writes
//!   up to a deadline, reports, signs out, then stops the runtime. The
//!   shell stops the engine only afterwards.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cl_core::ids::{CompanyId, RequestId};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{Notify, Semaphore, oneshot, watch};

use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::session::{Client, Scope, Session};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Queue limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueConfig {
    /// Jobs queued or running at once.
    pub max_outstanding: usize,
    /// Concurrent engine requests (reads and writes).
    pub max_concurrent: usize,
    /// Concurrent background prefetches.
    pub prefetch_concurrent: usize,
    /// Runtime worker threads.
    pub worker_threads: usize,
}

impl Default for QueueConfig {
    fn default() -> Self {
        QueueConfig {
            max_outstanding: 256,
            max_concurrent: 8,
            prefetch_concurrent: 2,
            worker_threads: 2,
        }
    }
}

/// Why a job was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QueueError {
    /// Too many outstanding jobs; try again next frame.
    #[error("request queue is full")]
    QueueFull,
    /// The queue is shutting down.
    #[error("request queue is closed")]
    QueueClosed,
    /// The runtime could not be started.
    #[error("request runtime could not start")]
    Runtime,
}

/// What a job's results belong to: the scope epoch and the company.
pub trait ScopeSource: Clone + Send + Sync + 'static {
    /// Current scope epoch (moves on every company switch).
    fn scope_epoch(&self) -> u64;
    /// The open company, if any.
    fn company(&self) -> Option<CompanyId>;
    /// The epoch, the company and a view pinned to them, read together. A
    /// job runs on the pinned view for its whole life.
    fn pinned(&self) -> (u64, Option<CompanyId>, Self) {
        (self.scope_epoch(), self.company(), self.clone())
    }
}

impl ScopeSource for Session {
    fn scope_epoch(&self) -> u64 {
        Session::scope_epoch(self)
    }

    fn company(&self) -> Option<CompanyId> {
        match self.scope() {
            Scope::Company { company_id, .. } => Some(company_id),
            Scope::Workspace => None,
        }
    }

    fn pinned(&self) -> (u64, Option<CompanyId>, Self) {
        let view = Session::pinned(self);
        let pin = view.pin_now();
        let company = match pin.scope {
            Scope::Company { company_id, .. } => Some(company_id),
            Scope::Workspace => None,
        };
        (pin.epoch, company, view)
    }
}

/// Before sign-in: one fixed scope.
impl ScopeSource for Client {
    fn scope_epoch(&self) -> u64 {
        0
    }

    fn company(&self) -> Option<CompanyId> {
        None
    }
}

/// Names a write for the owner if its window is gone. `what` is an i18n
/// key (`save.journal_entry`); never amounts, names or memo text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanLabel {
    /// i18n key of the action.
    pub what: &'static str,
    /// The idempotency key the save used ("Check again" needs it).
    pub key: Option<IdempotencyKey>,
}

/// How an orphaned write ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrphanResult {
    /// Saved.
    Succeeded,
    /// Failed, or `OutcomeUnknown`.
    Failed(ApiError),
}

/// The result of a write whose window closed or whose company was switched
/// away from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanOutcome {
    /// The write.
    pub label: OrphanLabel,
    /// The company it ran in.
    pub company: Option<CompanyId>,
    /// How it ended.
    pub result: OrphanResult,
    /// It finished after a company switch.
    pub stale_scope: bool,
}

/// What [`ApiQueue::shutdown`] saw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Writes that finished (saved or failed) while shutting down.
    pub completed: usize,
    /// Writes that finished with `OutcomeUnknown` while shutting down.
    pub outcome_unknown: usize,
    /// Writes still running at the deadline.
    pub abandoned: Vec<OrphanLabel>,
}

/// A job's result, polled by the UI every frame.
pub struct Pending<T> {
    rx: oneshot::Receiver<Result<T, ApiError>>,
    done: Arc<AtomicBool>,
    taken: bool,
    /// For a write: its idempotency key, so a lost result is reported as
    /// `OutcomeUnknown` (it may have been sent), never as `Internal`.
    write: IfLost,
}

/// What a job whose result channel was dropped resolves to.
#[derive(Clone)]
enum IfLost {
    /// A read: `Internal`.
    Internal,
    /// A write (with its key): `OutcomeUnknown`.
    Unknown(Option<IdempotencyKey>),
}

impl<T> std::fmt::Debug for Pending<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("done", &self.is_done())
            .finish_non_exhaustive()
    }
}

impl<T> Pending<T> {
    /// The result once, when it is there; never blocks.
    pub fn try_take(&mut self) -> Option<Result<T, ApiError>> {
        if self.taken {
            return None;
        }
        match self.rx.try_recv() {
            Ok(r) => {
                self.taken = true;
                Some(r)
            }
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => {
                self.taken = true;
                Some(Err(match &self.write {
                    IfLost::Unknown(key) => ApiError::OutcomeUnknown {
                        request_id: RequestId::from_uuid(uuid::Uuid::new_v4()),
                        idempotency_key: key.clone(),
                        // A job lost on the way: the block stays (Q-PR1a).
                        keyed: false,
                    },
                    IfLost::Internal => ApiError::Internal,
                }))
            }
        }
    }

    /// The job finished (its result may already have been taken).
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Read,
    Prefetch,
    Write,
}

struct Shared {
    notify: Arc<dyn Fn() + Send + Sync>,
    cfg: QueueConfig,
    outstanding: AtomicUsize,
    closed: AtomicBool,
    requests: Arc<Semaphore>,
    prefetch: Arc<Semaphore>,
    orphans: Mutex<Vec<OrphanOutcome>>,
    writes: Mutex<HashMap<u64, OrphanLabel>>,
    next_id: AtomicU64,
    write_done: Notify,
    /// (finished writes, of which `OutcomeUnknown`), since start.
    stats: Mutex<(usize, usize)>,
    reads_cancel: watch::Sender<bool>,
}

/// The request queue. One per app.
pub struct ApiQueue {
    shared: Arc<Shared>,
    runtime: Option<Runtime>,
}

impl std::fmt::Debug for ApiQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiQueue")
            .field("pending_writes", &self.pending_writes())
            .finish_non_exhaustive()
    }
}

impl ApiQueue {
    /// Starts the runtime. `notify` is called whenever a job finishes
    /// (cl-ui passes `ctx.request_repaint`).
    ///
    /// # Errors
    /// `Runtime` when the threads cannot be started.
    pub fn start(
        notify: Arc<dyn Fn() + Send + Sync>,
        cfg: QueueConfig,
    ) -> Result<ApiQueue, QueueError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(cfg.worker_threads.max(1))
            .thread_name("cl-api")
            .enable_all()
            .build()
            .map_err(|_| QueueError::Runtime)?;
        let (reads_cancel, _) = watch::channel(false);
        Ok(ApiQueue {
            shared: Arc::new(Shared {
                notify,
                cfg,
                outstanding: AtomicUsize::new(0),
                closed: AtomicBool::new(false),
                requests: Arc::new(Semaphore::new(cfg.max_concurrent.max(1))),
                prefetch: Arc::new(Semaphore::new(cfg.prefetch_concurrent.max(1))),
                orphans: Mutex::new(Vec::new()),
                writes: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                write_done: Notify::new(),
                stats: Mutex::new((0, 0)),
                reads_cancel,
            }),
            runtime: Some(runtime),
        })
    }

    /// A request group for one window or tab.
    ///
    /// # Errors
    /// `QueueClosed` after shutdown began.
    pub fn group(&self) -> Result<RequestGroup, QueueError> {
        let handle = self
            .runtime
            .as_ref()
            .map(|r| r.handle().clone())
            .ok_or(QueueError::QueueClosed)?;
        let (cancel, _) = watch::channel(false);
        Ok(RequestGroup {
            shared: self.shared.clone(),
            handle,
            cancel,
        })
    }

    /// Results of writes whose window closed or whose company changed.
    pub fn take_orphans(&self) -> Vec<OrphanOutcome> {
        std::mem::take(&mut *lock(&self.shared.orphans))
    }

    /// Writes still running (P08 guards company switch on this).
    #[must_use]
    pub fn pending_writes(&self) -> usize {
        lock(&self.shared.writes).len()
    }

    /// Orderly stop (S1). Call from a thread that is not inside a tokio
    /// runtime (the UI thread at exit, or a helper thread that keeps the
    /// UI painting "Finishing 1 save…" through `notify`/`pending_writes`).
    ///
    /// 1. Stops intake and cancels reads.
    /// 2. Waits for in-flight writes until `deadline`.
    /// 3. Reports; writes still running are `abandoned`.
    /// 4. Signs `session` out (best effort, 2 s).
    /// 5. Stops the runtime (2 s).
    ///
    /// The shell stops the engine only after this returns.
    #[must_use]
    pub fn shutdown(mut self, deadline: Duration, session: Option<Session>) -> ShutdownReport {
        let Some(rt) = self.runtime.take() else {
            return ShutdownReport::default();
        };
        let shared = self.shared.clone();
        shared.closed.store(true, Ordering::SeqCst);
        shared.reads_cancel.send_replace(true);
        let before = *lock(&shared.stats);
        rt.block_on(async {
            let end = tokio::time::Instant::now() + deadline;
            loop {
                let notified = shared.write_done.notified();
                if lock(&shared.writes).is_empty() {
                    break;
                }
                if tokio::time::timeout_at(end, notified).await.is_err() {
                    break;
                }
            }
        });
        let after = *lock(&shared.stats);
        let abandoned = lock(&shared.writes).values().cloned().collect();
        let report = ShutdownReport {
            completed: after.0 - before.0,
            outcome_unknown: after.1 - before.1,
            abandoned,
        };
        if let Some(s) = session {
            let _ = rt.block_on(tokio::time::timeout(Duration::from_secs(2), s.logout()));
        }
        rt.shutdown_timeout(Duration::from_secs(2));
        tracing::info!(
            completed = report.completed,
            outcome_unknown = report.outcome_unknown,
            abandoned = report.abandoned.len(),
            "request queue stopped"
        );
        report
    }
}

impl Drop for ApiQueue {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            tracing::warn!("request queue dropped without shutdown");
            self.shared.closed.store(true, Ordering::SeqCst);
            if Handle::try_current().is_ok() {
                rt.shutdown_background();
            } else {
                rt.shutdown_timeout(Duration::from_secs(2));
            }
        }
    }
}

/// The jobs of one window. Dropping it cancels its reads.
pub struct RequestGroup {
    shared: Arc<Shared>,
    handle: Handle,
    cancel: watch::Sender<bool>,
}

impl std::fmt::Debug for RequestGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestGroup").finish_non_exhaustive()
    }
}

async fn cancelled(rx: &mut watch::Receiver<bool>) {
    // A dropped sender (the group or queue is gone) also cancels.
    let _ = rx.wait_for(|c| *c).await;
}

fn group_gone(rx: &watch::Receiver<bool>) -> bool {
    *rx.borrow() || rx.has_changed().is_err()
}

impl RequestGroup {
    /// Runs a read. Its result is dropped (`StaleScope`) when the company
    /// changed meanwhile, and it is aborted (`Cancelled`) when the group is.
    ///
    /// # Errors
    /// `QueueFull` or `QueueClosed`; nothing was started.
    pub fn read<S, T, F, Fut>(&self, scope: &S, f: F) -> Result<Pending<T>, QueueError>
    where
        S: ScopeSource,
        T: Send + 'static,
        F: FnOnce(S) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        self.spawn(Kind::Read, scope, None, f)
    }

    /// A background read on the prefetch lane (2 at a time).
    ///
    /// # Errors
    /// `QueueFull` or `QueueClosed`.
    pub fn prefetch<S, T, F, Fut>(&self, scope: &S, f: F) -> Result<Pending<T>, QueueError>
    where
        S: ScopeSource,
        T: Send + 'static,
        F: FnOnce(S) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        self.spawn(Kind::Prefetch, scope, None, f)
    }

    /// Runs a write. It is never aborted; if its window closes or the
    /// company changes, its result also goes to the orphans.
    ///
    /// # Errors
    /// `QueueFull` or `QueueClosed`; nothing was started or sent.
    pub fn write<S, T, F, Fut>(
        &self,
        scope: &S,
        label: OrphanLabel,
        f: F,
    ) -> Result<Pending<T>, QueueError>
    where
        S: ScopeSource,
        T: Send + 'static,
        F: FnOnce(S) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        self.spawn(Kind::Write, scope, Some(label), f)
    }

    /// Cancels the group's reads; its writes keep running (orphans).
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    fn spawn<S, T, F, Fut>(
        &self,
        kind: Kind,
        scope: &S,
        label: Option<OrphanLabel>,
        f: F,
    ) -> Result<Pending<T>, QueueError>
    where
        S: ScopeSource,
        T: Send + 'static,
        F: FnOnce(S) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        let shared = self.shared.clone();
        if shared.closed.load(Ordering::SeqCst) {
            return Err(QueueError::QueueClosed);
        }
        if shared.outstanding.fetch_add(1, Ordering::SeqCst) >= shared.cfg.max_outstanding {
            shared.outstanding.fetch_sub(1, Ordering::SeqCst);
            return Err(QueueError::QueueFull);
        }
        let (tx, rx) = oneshot::channel();
        let done = Arc::new(AtomicBool::new(false));
        let write_key = label
            .as_ref()
            .map_or(IfLost::Internal, |l| IfLost::Unknown(l.key.clone()));
        // The job runs pinned to the company open now; the orphan record
        // names that company (the one the requests actually used).
        let (epoch, company, job_scope) = scope.pinned();
        let checker = scope.clone();

        let mut group_rx = self.cancel.subscribe();
        let mut global_rx = shared.reads_cancel.subscribe();
        let write_id = label.as_ref().map(|l| {
            let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
            lock(&shared.writes).insert(id, l.clone());
            id
        });
        let sem = if kind == Kind::Prefetch {
            shared.prefetch.clone()
        } else {
            shared.requests.clone()
        };
        let job = self.handle.spawn(async move {
            let _permit = sem.acquire_owned().await;
            f(job_scope).await
        });
        let done_flag = done.clone();
        self.handle.spawn(async move {
            let result = if kind == Kind::Write {
                match job.await {
                    Ok(r) => r,
                    // A panicked or aborted write may have been sent.
                    Err(_) => Err(ApiError::OutcomeUnknown {
                        request_id: RequestId::from_uuid(uuid::Uuid::new_v4()),
                        idempotency_key: label.as_ref().and_then(|l| l.key.clone()),
                        keyed: false,
                    }),
                }
            } else {
                let abort = job.abort_handle();
                tokio::select! {
                    r = job => r.unwrap_or(Err(ApiError::Internal)),
                    () = cancelled(&mut group_rx) => { abort.abort(); Err(ApiError::Cancelled) }
                    () = cancelled(&mut global_rx) => { abort.abort(); Err(ApiError::Cancelled) }
                }
            };
            let stale = checker.scope_epoch() != epoch;
            let result = match (kind, stale) {
                (Kind::Read | Kind::Prefetch, true) => Err(ApiError::StaleScope),
                _ => result,
            };
            if let (Some(label), Some(id)) = (label, write_id) {
                if stale || group_gone(&group_rx) || tx.is_closed() {
                    lock(&shared.orphans).push(OrphanOutcome {
                        label,
                        company,
                        result: match &result {
                            Ok(_) => OrphanResult::Succeeded,
                            Err(e) => OrphanResult::Failed(e.clone()),
                        },
                        stale_scope: stale,
                    });
                }
                {
                    let mut st = lock(&shared.stats);
                    st.0 += 1;
                    if matches!(result, Err(ApiError::OutcomeUnknown { .. })) {
                        st.1 += 1;
                    }
                }
                lock(&shared.writes).remove(&id);
            }
            done_flag.store(true, Ordering::Release);
            let _ = tx.send(result);
            shared.outstanding.fetch_sub(1, Ordering::SeqCst);
            shared.write_done.notify_waiters();
            (shared.notify)();
        });
        Ok(Pending {
            rx,
            done,
            taken: false,
            write: write_key,
        })
    }
}

impl Drop for RequestGroup {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
    }
}

#[cfg(test)]
#[allow(clippy::many_single_char_names, reason = "short test bindings")]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A scope whose epoch the test moves.
    #[derive(Clone)]
    struct FakeScope(Arc<AtomicU64>);

    impl ScopeSource for FakeScope {
        fn scope_epoch(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
        fn company(&self) -> Option<CompanyId> {
            CompanyId::parse("0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").ok()
        }
    }

    fn scope() -> FakeScope {
        FakeScope(Arc::new(AtomicU64::new(1)))
    }

    fn queue(cfg: QueueConfig) -> (ApiQueue, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let q = ApiQueue::start(
            Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }),
            cfg,
        )
        .expect("queue");
        (q, n)
    }

    fn wait<T>(p: &mut Pending<T>) -> Result<T, ApiError> {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(r) = p.try_take() {
                return r;
            }
            assert!(Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn label() -> OrphanLabel {
        OrphanLabel {
            what: "save.test",
            key: Some(IdempotencyKey::new()),
        }
    }

    #[test]
    fn handles_never_block_and_notify_on_completion() {
        let (q, notified) = queue(QueueConfig::default());
        let g = q.group().expect("group");
        let mut p = g
            .read(&scope(), |_| async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(7)
            })
            .expect("queued");
        let t = Instant::now();
        assert!(p.try_take().is_none());
        assert!(!p.is_done());
        assert!(
            t.elapsed() < Duration::from_millis(20),
            "try_take is instant"
        );
        assert_eq!(wait(&mut p), Ok(7));
        assert!(p.is_done());
        assert!(p.try_take().is_none(), "taken once");
        assert!(notified.load(Ordering::SeqCst) >= 1);
        drop(g);
        assert_eq!(
            q.shutdown(Duration::from_secs(1), None),
            ShutdownReport::default()
        );
    }

    #[test]
    fn a_lost_write_result_is_outcome_unknown_and_a_lost_read_internal() {
        let key = IdempotencyKey::new();
        let (tx, rx) = oneshot::channel::<Result<(), ApiError>>();
        drop(tx);
        let mut w = Pending {
            rx,
            done: Arc::new(AtomicBool::new(true)),
            taken: false,
            write: IfLost::Unknown(Some(key.clone())),
        };
        match w.try_take() {
            Some(Err(ApiError::OutcomeUnknown {
                idempotency_key, ..
            })) => assert_eq!(idempotency_key, Some(key)),
            other => panic!("unexpected {other:?}"),
        }
        let (tx, rx) = oneshot::channel::<Result<(), ApiError>>();
        drop(tx);
        let mut r = Pending {
            rx,
            done: Arc::new(AtomicBool::new(true)),
            taken: false,
            write: IfLost::Internal,
        };
        assert_eq!(r.try_take(), Some(Err(ApiError::Internal)));
    }

    #[test]
    fn backpressure_refuses_beyond_the_cap() {
        let (q, _) = queue(QueueConfig {
            max_outstanding: 2,
            ..QueueConfig::default()
        });
        let g = q.group().expect("group");
        let s = scope();
        let slow = |_| async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(())
        };
        let _a = g.read(&s, slow).expect("1");
        let _b = g.read(&s, slow).expect("2");
        assert_eq!(g.read(&s, slow).map(drop), Err(QueueError::QueueFull));
        drop(g);
        let _ = q.shutdown(Duration::from_millis(100), None);
    }

    #[test]
    fn cancel_aborts_reads_but_detaches_writes_to_the_orphans() {
        let (q, _) = queue(QueueConfig::default());
        let g = q.group().expect("group");
        let s = scope();
        let finished = Arc::new(AtomicBool::new(false));
        let flag = finished.clone();
        let mut read = g
            .read(&s, move |_| async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
                flag.store(true, Ordering::SeqCst);
                Ok(())
            })
            .expect("read");
        let (release, gate) = oneshot::channel::<()>();
        let write = g
            .write(&s, label(), move |_| async move {
                let _ = gate.await;
                Ok("saved")
            })
            .expect("write");
        assert_eq!(q.pending_writes(), 1);
        g.cancel();
        assert_eq!(wait(&mut read), Err(ApiError::Cancelled));
        assert!(!finished.load(Ordering::SeqCst));
        drop(write); // the window closes
        drop(g);
        let _ = release.send(());
        let end = Instant::now() + Duration::from_secs(5);
        while q.pending_writes() > 0 {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
        let orphans = q.take_orphans();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].result, OrphanResult::Succeeded);
        assert_eq!(orphans[0].label.what, "save.test");
        assert!(!orphans[0].stale_scope);
        assert_eq!(q.take_orphans(), Vec::new());
        let _ = q.shutdown(Duration::from_secs(1), None);
    }

    #[test]
    fn stale_scope_drops_reads_and_reports_writes() {
        let (q, _) = queue(QueueConfig::default());
        let g = q.group().expect("group");
        let s = scope();
        let (r_go, r_gate) = oneshot::channel::<()>();
        let (w_go, w_gate) = oneshot::channel::<()>();
        let mut read = g
            .read(&s, move |_| async move {
                let _ = r_gate.await;
                Ok(1)
            })
            .expect("read");
        let mut write = g
            .write(&s, label(), move |_| async move {
                let _ = w_gate.await;
                Err::<(), _>(ApiError::Timeout)
            })
            .expect("write");
        s.0.store(2, Ordering::SeqCst); // company switch
        let _ = r_go.send(());
        let _ = w_go.send(());
        assert_eq!(wait(&mut read), Err(ApiError::StaleScope));
        assert_eq!(wait(&mut write), Err(ApiError::Timeout), "not dropped");
        let orphans = q.take_orphans();
        assert_eq!(orphans.len(), 1);
        assert!(orphans[0].stale_scope);
        assert_eq!(orphans[0].company, s.company());
        assert_eq!(orphans[0].result, OrphanResult::Failed(ApiError::Timeout));
        drop(g);
        let _ = q.shutdown(Duration::from_secs(1), None);
    }

    #[test]
    fn panics_become_outcome_unknown_for_writes_and_internal_for_reads() {
        let (q, _) = queue(QueueConfig::default());
        let g = q.group().expect("group");
        let s = scope();
        let l = label();
        let key = l.key.clone();
        let boom = || -> Result<(), ApiError> {
            panic!("job panicked (expected in this test)");
        };
        let mut w = g
            .write(&s, l, move |_| async move { boom() })
            .expect("write");
        let mut r = g.read(&s, move |_| async move { boom() }).expect("read");
        match wait(&mut w) {
            Err(ApiError::OutcomeUnknown {
                idempotency_key, ..
            }) => assert_eq!(idempotency_key, key),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(wait(&mut r), Err(ApiError::Internal));
        drop(g);
        let _ = q.shutdown(Duration::from_secs(1), None);
    }

    #[test]
    fn shutdown_waits_for_writes_cancels_reads_and_reports_abandoned() {
        let (q, _) = queue(QueueConfig::default());
        let g = q.group().expect("group");
        let s = scope();
        let mut w = g
            .write(&s, label(), |_| async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(1)
            })
            .expect("write");
        let mut r = g
            .read(&s, |_| async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(2)
            })
            .expect("read");
        let report = q.shutdown(Duration::from_secs(5), None);
        assert_eq!(report.completed, 1);
        assert_eq!(report.outcome_unknown, 0);
        assert_eq!(report.abandoned, Vec::new());
        assert_eq!(wait(&mut w), Ok(1), "the save finished before the stop");
        assert_eq!(wait(&mut r), Err(ApiError::Cancelled));
        assert_eq!(
            g.read(&s, |_| async { Ok(()) }).map(drop),
            Err(QueueError::QueueClosed)
        );

        let (q, _) = queue(QueueConfig::default());
        let g = q.group().expect("group");
        let _slow = g
            .write(&s, label(), |_| async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(())
            })
            .expect("write");
        let t = Instant::now();
        let report = q.shutdown(Duration::from_millis(100), None);
        assert_eq!(report.abandoned.len(), 1);
        assert_eq!(report.abandoned[0].what, "save.test");
        assert!(t.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn a_write_queued_before_a_switch_sends_nothing_and_its_orphan_says_stale() {
        use crate::client::{Call, Method};
        use crate::resources::operations::find;
        use crate::resources::testkit::{COMPANY, company_session, json};
        use cl_core::ids::TenantId;
        use wiremock::{Mock, MockServer};
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let server = rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(wiremock::matchers::any())
                .respond_with(json(201, "{}"))
                .mount(&server)
                .await;
            server
        });
        let session = company_session(&server);
        let (q, _) = queue(QueueConfig {
            max_concurrent: 1,
            ..QueueConfig::default()
        });
        let g = q.group().expect("group");
        // Hold the only request slot, and wait until it is held, so the
        // second write is queued behind it rather than racing it.
        let (release, gate) = oneshot::channel::<()>();
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let _blocker = g
            .write(&session, label(), move |_| async move {
                let _ = held_tx.send(());
                let _ = gate.await;
                Ok(())
            })
            .expect("blocker");
        held_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the blocker holds the slot");
        let l = OrphanLabel {
            what: "save.pinned",
            key: None,
        };
        let mut w = g
            .write(&session, l, |s: Session| async move {
                let op = *find(Method::Post, "/api/v1/accounts").expect("op");
                let call = Call::at(Method::Post, op.route()).json_body(b"{}".to_vec());
                s.send(&op, call, None).await.map(drop)
            })
            .expect("write");
        session.set_scope(Scope::Company {
            tenant_id: TenantId::parse("1c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("t"),
            company_id: CompanyId::parse("0c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("c"),
        });
        let _ = release.send(());
        assert_eq!(wait(&mut w), Err(ApiError::StaleScope));
        let sent = rt.block_on(server.received_requests()).expect("recorded");
        assert_eq!(sent.len(), 0, "nothing reached the engine");
        let orphans = q.take_orphans();
        let o = orphans
            .iter()
            .find(|o| o.label.what == "save.pinned")
            .expect("reported");
        assert_eq!(o.result, OrphanResult::Failed(ApiError::StaleScope));
        assert_eq!(
            o.company,
            CompanyId::parse(COMPANY).ok(),
            "the queued company"
        );
        assert!(o.stale_scope);
        drop(g);
        let _ = q.shutdown(Duration::from_secs(1), None);
    }

    #[test]
    fn drives_the_client_against_a_stub_engine() {
        use crate::resources::testkit::{company_session, json};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer};
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let server = rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/v1/ledger-version"))
                .respond_with(json(200, r#"{"stamp":3}"#))
                .mount(&server)
                .await;
            server
        });
        let session = company_session(&server);
        let (q, _) = queue(QueueConfig::default());
        let g = q.group().expect("group");
        let mut p = g
            .read(&session, |s| async move { s.ledger_version().await })
            .expect("read");
        let first = wait(&mut p).expect("stamp");
        let mut p = g
            .read(&session, |s| async move { s.ledger_version().await })
            .expect("read");
        assert_eq!(wait(&mut p).expect("stamp"), first);
        let mut c = g
            .read(
                session.client(),
                |c| async move { c.health().await.map(drop) },
            )
            .expect("client scope");
        assert!(
            wait(&mut c).is_err(),
            "no /health mock: an error, not a hang"
        );
        drop(g);
        let _ = q.shutdown(Duration::from_secs(1), None);
    }
}
