//! What the company screens share: the engine client, the request queue,
//! the signed-in session, the local stores, and a mailbox of events for
//! the shell (company opened or closed, sign-in). In `--demo` there is no
//! engine: the screens show the mockup's synthetic data and every action
//! completes at once, which is what the control-channel scenarios drive.
//!
//! Every engine call runs on the `cl-api` queue and is polled each frame
//! through [`Task`]; the egui thread never blocks.

pub mod demo;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use cl_api::{
    ApiQueue, Capabilities, Client, OrphanLabel, Pending, QueueError, RequestGroup, Session,
};
use cl_core::ids::TenantId;
use cl_host::Supervisor;
use cl_settings::{Accent, RecentStore, SecretStore};

/// Something the shell must react to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompanyEvent {
    /// The owner signed in.
    SignedIn {
        /// Display text for the status bar (`Kurt · Admin`).
        user: String,
    },
    /// A company was opened or switched to.
    Opened {
        /// The tenant.
        tenant_id: TenantId,
        /// Name for the title bar.
        name: String,
        /// Stripe colour.
        accent: Accent,
        /// Closing date, when set.
        closed_through: Option<cl_core::CalendarDate>,
    },
    /// No company is open now.
    Closed,
    /// Open a window (a screen asked for another screen, for example the
    /// interview opening the recovery key).
    OpenWindow(cl_model::CommandId),
    /// A short confirmation for the toast line (i18n key).
    Toast(&'static str),
    /// Take this toast (i18n key) off the status bar now, if it is the
    /// one showing (P15: a refusal the owner has since fixed).
    ClearToast(&'static str),
    /// A company switch was refused because a window has unsaved edits:
    /// the shell shows the close prompt for the first one.
    PromptDirty,
    /// Close the window of this kind (a screen that finished its job).
    CloseWindow(cl_model::CommandId),
    /// Bring the register of this account to the front, or open it in its
    /// own tab (one register tab per account, P12).
    OpenRegister(cl_core::ids::AccountId),
    /// Open this saved transaction in the form of this kind (P14: a grouped
    /// deposit or a daily-sales day opened from a register row, Q-P14g).
    OpenRecord(cl_model::CommandId, cl_core::ids::TransactionId),
    /// Open a fresh form filled from a template (P15: Templates "Use",
    /// Templates Due "Open in form").
    OpenTemplate(Box<crate::forms::template::TemplateOpen>),
    /// Open a fresh window of this kind with what another screen handed
    /// over (P17: a document's full-size tab, a bill prefilled from a
    /// statement line, a receipt-inbox file). A screen that does not take
    /// the payload just opens (or comes to the front) as usual.
    OpenWith(cl_model::CommandId, Box<crate::windows::OpenPayload>),
    /// The owner saved new user preferences (theme, zoom, dates): the
    /// shell applies and persists them.
    UserPrefs(cl_settings::UserPrefs),
    /// Home read the rail's counts (clients, vendors, bank items waiting;
    /// P16). Counts only.
    RailCounts(crate::shell::RailCounts),
}

struct Live {
    client: Client,
    queue: Arc<ApiQueue>,
    supervisor: Arc<Supervisor>,
}

/// The engine as the company screens see it.
pub struct CompanyLink {
    live: Option<Live>,
    store: RecentStore,
    secrets: Arc<dyn SecretStore>,
    session: Mutex<Option<Session>>,
    events: Mutex<Vec<CompanyEvent>>,
    dirty_windows: std::sync::atomic::AtomicUsize,
    /// Lifecycle operations (create, open, restore, backup, delete, sample
    /// load) still running; a company switch waits for zero.
    lifecycle_running: Arc<std::sync::atomic::AtomicUsize>,
    /// A recovery key the engine returned at first-run setup (U-23), kept
    /// only until the recovery-key screen shows it once.
    recovery_key: Mutex<Option<cl_core::Secret>>,
    /// Default folder for backups and exports.
    pub backup_dir: PathBuf,
    /// The demo list and state, in demo mode.
    demo: Option<Mutex<demo::DemoState>>,
    /// "Data changed" notices between windows (Q-262).
    bus: crate::data_bus::DataBus,
    /// The last catch-up run's answer, shared by the runner and the
    /// Templates Due window (P15).
    due: Mutex<crate::templates_runner::DueBoard>,
    /// Saves whose outcome is unknown, for the whole app session, so the
    /// block outlives the window that made them (review fix-PR-1 B2).
    lost_saves: Mutex<crate::forms::lost_save::LostBook>,
}

impl std::fmt::Debug for CompanyLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompanyLink")
            .field("demo", &self.demo.is_some())
            .finish_non_exhaustive()
    }
}

impl CompanyLink {
    /// The real thing: a client over the supervisor's endpoint, the request
    /// queue, and the stores under the app data folder.
    #[must_use]
    pub fn live(
        client: Client,
        queue: ApiQueue,
        supervisor: Arc<Supervisor>,
        store: RecentStore,
        secrets: Arc<dyn SecretStore>,
        backup_dir: PathBuf,
    ) -> CompanyLink {
        CompanyLink {
            live: Some(Live {
                client,
                queue: Arc::new(queue),
                supervisor,
            }),
            store,
            secrets,
            session: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            dirty_windows: std::sync::atomic::AtomicUsize::new(0),
            lifecycle_running: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            recovery_key: Mutex::new(None),
            backup_dir,
            demo: None,
            bus: crate::data_bus::DataBus::new(),
            due: Mutex::new(crate::templates_runner::DueBoard::default()),
            lost_saves: Mutex::new(crate::forms::lost_save::LostBook::default()),
        }
    }

    /// No engine: the mockup's synthetic companies, every action succeeds at
    /// once. The store lives in `dir` (a temp folder in `ui-check`).
    #[must_use]
    pub fn demo(dir: &std::path::Path) -> CompanyLink {
        CompanyLink {
            live: None,
            store: RecentStore::in_dir(&dir.join("settings")),
            secrets: Arc::new(cl_settings::MemorySecrets::new()),
            session: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            dirty_windows: std::sync::atomic::AtomicUsize::new(0),
            lifecycle_running: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            recovery_key: Mutex::new(None),
            backup_dir: dir.join("Backups"),
            demo: Some(Mutex::new(demo::DemoState::new())),
            bus: crate::data_bus::DataBus::new(),
            due: Mutex::new(crate::templates_runner::DueBoard::default()),
            lost_saves: Mutex::new(crate::forms::lost_save::LostBook::default()),
        }
    }

    /// True without an engine.
    #[must_use]
    pub fn is_demo(&self) -> bool {
        self.live.is_none()
    }

    /// The demo state (None when live).
    pub fn demo_state(&self) -> Option<std::sync::MutexGuard<'_, demo::DemoState>> {
        self.demo
            .as_ref()
            .map(|m| m.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The engine client (None in demo).
    #[must_use]
    pub fn client(&self) -> Option<&Client> {
        self.live.as_ref().map(|l| &l.client)
    }

    /// The supervisor, for first-run setup (None in demo).
    #[must_use]
    pub fn supervisor(&self) -> Option<&Arc<Supervisor>> {
        self.live.as_ref().map(|l| &l.supervisor)
    }

    /// A request group for one window.
    ///
    /// # Errors
    /// `QueueClosed` when the app is shutting down; `None` in demo.
    pub fn group(&self) -> Option<Result<RequestGroup, QueueError>> {
        self.live.as_ref().map(|l| l.queue.group())
    }

    /// Writes still running (a company switch waits for zero).
    #[must_use]
    pub fn pending_writes(&self) -> usize {
        self.live.as_ref().map_or(0, |l| l.queue.pending_writes())
    }

    /// Windows with unsaved edits (the shell updates this every frame; a
    /// company switch refuses while it is above zero, Q-182).
    pub fn set_dirty_windows(&self, n: usize) {
        self.dirty_windows
            .store(n, std::sync::atomic::Ordering::Relaxed);
    }

    /// See [`CompanyLink::set_dirty_windows`].
    #[must_use]
    pub fn dirty_windows(&self) -> usize {
        self.dirty_windows
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Lifecycle operations still running (see [`spawn_lifecycle`]).
    #[must_use]
    pub fn lifecycle_running(&self) -> usize {
        self.lifecycle_running
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// True while a company may not be switched: a lifecycle operation or
    /// a write is still running.
    #[must_use]
    pub fn switch_blocked(&self) -> bool {
        self.lifecycle_running() > 0 || self.pending_writes() > 0
    }

    /// Keep a recovery key from first-run setup for the screen that shows it.
    pub fn stash_recovery_key(&self, key: cl_core::Secret) {
        *self
            .recovery_key
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(key);
    }

    /// True while a first-run recovery key waits to be shown.
    #[must_use]
    pub fn has_recovery_key(&self) -> bool {
        self.recovery_key
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Takes the stashed recovery key (once).
    #[must_use]
    pub fn take_recovery_key(&self) -> Option<cl_core::Secret> {
        self.recovery_key
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// The signed-in session, if any.
    #[must_use]
    pub fn session(&self) -> Option<Session> {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replace the session (after sign-in or sign-out).
    pub fn set_session(&self, s: Option<Session>) {
        *self.session.lock().unwrap_or_else(PoisonError::into_inner) = s;
    }

    /// Capabilities of the engine (empty in demo).
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        self.live
            .as_ref()
            .map(|l| l.client.capabilities())
            .unwrap_or_default()
    }

    /// The recent-companies store.
    #[must_use]
    pub fn store(&self) -> &RecentStore {
        &self.store
    }

    /// The secret store.
    #[must_use]
    pub fn secrets(&self) -> Arc<dyn SecretStore> {
        Arc::clone(&self.secrets)
    }

    /// The "data changed" notices (Q-262): every successful save, void or
    /// delete publishes here; windows showing the data subscribe.
    #[must_use]
    pub fn bus(&self) -> &crate::data_bus::DataBus {
        &self.bus
    }

    /// The catch-up board (the last run's answer, "Check now"; P15).
    pub fn due(&self) -> std::sync::MutexGuard<'_, crate::templates_runner::DueBoard> {
        self.due.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The unknown saves of this app session (see
    /// [crate::forms::lost_save::LostBook]).
    pub fn lost_saves(&self) -> std::sync::MutexGuard<'_, crate::forms::lost_save::LostBook> {
        self.lost_saves
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue an event for the shell.
    pub fn push(&self, e: CompanyEvent) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(e);
    }

    /// Take the queued events (the shell, once per frame).
    #[must_use]
    pub fn drain(&self) -> Vec<CompanyEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// A queued job the screen polls each frame, or an answer the demo gave
/// at once.
pub enum Task<T> {
    /// Running on the queue.
    Pending(Pending<T>),
    /// Already answered (demo, or a local refusal).
    Ready(Option<T>),
}

impl<T> std::fmt::Debug for Task<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task").finish_non_exhaustive()
    }
}

impl<T> Task<T> {
    /// An answer available now.
    #[must_use]
    pub fn ready(value: T) -> Task<T> {
        Task::Ready(Some(value))
    }

    /// The result once; `None` while still running (or after it was taken).
    /// A queue failure becomes `Err(ApiError)` inside the value when `T`
    /// is a `Result`, through [`Task::poll_result`].
    pub fn try_take(&mut self) -> Option<Result<T, cl_api::ApiError>> {
        match self {
            Task::Pending(p) => p.try_take(),
            Task::Ready(v) => v.take().map(Ok),
        }
    }
}

impl<T, E> Task<Result<T, E>>
where
    E: From<cl_api::ApiError>,
{
    /// The result once, with a queue failure folded into the error type.
    pub fn poll_result(&mut self) -> Option<Result<T, E>> {
        match self.try_take()? {
            Ok(inner) => Some(inner),
            Err(e) => Some(Err(E::from(e))),
        }
    }
}

/// Counts a running lifecycle operation for as long as it lives.
struct Running(Arc<std::sync::atomic::AtomicUsize>);

impl Running {
    fn start(counter: &Arc<std::sync::atomic::AtomicUsize>) -> Running {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Running(Arc::clone(counter))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Spawn `f` (a future over a session) on `group` as a **write** on the
/// client scope: it is never cancelled, `pending_writes()` stays above
/// zero until it ends (so a company switch waits), and the link counts
/// it as a running lifecycle operation. Ordinary reads inside a company
/// use the session scope instead.
///
/// # Errors
/// `QueueFull` or `QueueClosed`.
pub fn spawn_lifecycle<T, F, Fut>(
    link: &Arc<CompanyLink>,
    group: &RequestGroup,
    f: F,
) -> Result<Task<T>, QueueError>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
{
    let client = link.client().ok_or(QueueError::QueueClosed)?.clone();
    let running = Running::start(&link.lifecycle_running);
    let label = OrphanLabel {
        what: "orphan.lifecycle",
        key: None,
    };
    let pending = group.write(&client, label, move |_c| async move {
        let _running = running;
        Ok(f().await)
    })?;
    Ok(Task::Pending(pending))
}

/// Status-bar text for the signed-in owner (name and role; no email).
#[must_use]
pub fn user_text(display_name: &str, role: &str) -> String {
    let role = match role {
        "owner" | "super_admin" | "admin" => "Admin",
        other => other,
    };
    format!("{display_name} \u{b7} {role}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_link_has_no_engine_and_queues_events() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = CompanyLink::demo(dir.path());
        assert!(link.is_demo());
        assert!(link.client().is_none());
        assert!(link.group().is_none());
        assert_eq!(link.pending_writes(), 0);
        assert_eq!(link.lifecycle_running(), 0);
        assert!(!link.switch_blocked());
        assert!(link.session().is_none());
        link.push(CompanyEvent::Closed);
        assert_eq!(link.drain(), vec![CompanyEvent::Closed]);
        assert_eq!(link.drain().len(), 0);
        let mut t: Task<u8> = Task::ready(7);
        assert_eq!(t.try_take().map(Result::ok), Some(Some(7)));
        assert!(t.try_take().is_none());
        assert_eq!(user_text("Kurt", "owner"), "Kurt \u{b7} Admin");
    }
}
