//! What the company screens share: the engine client, the request queue,
//! the signed-in session, the local stores, and a mailbox of events for
//! the shell (company opened or closed, sign-in). In `--demo` there is no
//! engine: the screens show the mockup's synthetic data and every action
//! completes at once, which is what the control-channel scenarios drive.
//!
//! Every engine call runs on the `cl-api` queue and is polled each frame
//! through [`Task`]; the egui thread never blocks.

pub mod demo;

use std::path::{Path, PathBuf};
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
        /// The signed-in user's display name (no email). The role comes
        /// with the company ([`CompanyEvent::CompanyRole`]).
        user: String,
    },
    /// The signed-in user's role in the company being opened (the engine's
    /// word: `owner`, `admin`, `bookkeeper`, …), sent just before
    /// [`CompanyEvent::Opened`] (A09 QA-9: the status bar said "Admin" for
    /// every user).
    CompanyRole(String),
    /// The signed-in user's rights in the company being opened (P29),
    /// sent before [`CompanyEvent::Opened`] and again whenever they are
    /// read anew (a 403 from the engine). The shell re-filters its menus,
    /// rail and palette; screens read [`CompanyLink::rights`].
    Rights(cl_model::permissions::Rights),
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
    /// The closing date was set, moved or cleared (P29 Set Closing Date):
    /// the title bar follows.
    ClosingDate(Option<cl_core::CalendarDate>),
    /// No company is open now.
    Closed,
    /// Open a window (a screen asked for another screen, for example the
    /// interview opening the recovery key).
    OpenWindow(cl_model::CommandId),
    /// A short confirmation for the toast line (i18n key).
    Toast(&'static str),
    /// A toast whose words are already filled in (P24: the NEGATIVE_STOCK
    /// warning names the item, its date and on hand). Shown longer.
    ToastText(String),
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
    /// Company › My Sign-in chose a new idle-lock time (P29, Q-P29p): the
    /// shell changes only that field of its own preferences.
    IdleLockMinutes(u16),
    /// Home read the rail's counts (clients, vendors, bank items waiting;
    /// P16). Counts only.
    RailCounts(crate::shell::RailCounts),
    /// A bank file was sent to the Bank Feed or taken back out: the rail's
    /// badge shows the new count of items waiting (P25). A count only.
    BankFeedCount(u32),
    /// Q-PR4d: the name a Quick Add prompt sent to Set Up was saved: the
    /// Set Up window closes and the form that asked comes to the front.
    SetUpSaved,
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
    /// Messages waiting in the Outbox (P20, Q-P20d), as last read; shown on
    /// Home and in the status bar.
    outbox: Mutex<Option<u32>>,
    /// The signed-in user's rights in the open company (P29). Everything
    /// while no company is open (nothing but Open Company works then).
    rights: Mutex<cl_model::permissions::Rights>,
    /// The company open now (P29 WP7: its local tag rules).
    open_tenant: Mutex<Option<TenantId>>,
    /// P29 WP2: the signed-in email, kept in memory only (the idle unlock
    /// signs in again with it; never logged).
    sign_in_email: Mutex<Option<String>>,
    /// P29 WP2: Sign out skips automatic sign-in for the rest of the run.
    skip_auto_sign_in: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for CompanyLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompanyLink")
            .field("demo", &self.demo.is_some())
            .finish_non_exhaustive()
    }
}

impl CompanyLink {
    /// ClientLedger's own folders, never an intake folder (A01, security
    /// review L6): the settings folder, the default and the chosen backup
    /// folders, and (with an engine) the whole app data folder, which holds
    /// the `PostgreSQL` data, the engine's files and the logs. The demo's
    /// folders sit beside its settings, so only its settings folder counts.
    #[must_use]
    pub fn app_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        let settings = self.store.path().parent().map(Path::to_path_buf);
        if let Some(s) = &settings {
            dirs.push(s.clone());
            if self.live.is_some()
                && let Some(root) = s.parent()
            {
                dirs.push(root.to_path_buf());
            }
        }
        dirs.push(self.backup_dir.clone());
        if let Some(chosen) = self.store.load().ok().and_then(|l| l.backup_folder) {
            dirs.push(chosen);
        }
        dirs
    }

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
            outbox: Mutex::new(None),
            due: Mutex::new(crate::templates_runner::DueBoard::default()),
            lost_saves: Mutex::new(crate::forms::lost_save::LostBook::default()),
            rights: Mutex::new(cl_model::permissions::Rights::all()),
            open_tenant: Mutex::new(None),
            sign_in_email: Mutex::new(None),
            skip_auto_sign_in: std::sync::atomic::AtomicBool::new(false),
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
            outbox: Mutex::new(None),
            due: Mutex::new(crate::templates_runner::DueBoard::default()),
            lost_saves: Mutex::new(crate::forms::lost_save::LostBook::default()),
            rights: Mutex::new(cl_model::permissions::Rights::all()),
            open_tenant: Mutex::new(None),
            sign_in_email: Mutex::new(None),
            skip_auto_sign_in: std::sync::atomic::AtomicBool::new(false),
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

    /// Refreshes table statistics after a company is created, imported or
    /// restored (Q-335); nothing in demo. Holds the supervisor weakly, so a
    /// running task never keeps it alive at shutdown.
    #[must_use]
    pub fn statistics(&self) -> cl_company::Statistics {
        let Some(sup) = self.supervisor() else {
            return cl_company::Statistics::none();
        };
        let sup = Arc::downgrade(sup);
        cl_company::Statistics::new(move || {
            let sup = sup.upgrade().ok_or("the app is shutting down")?;
            sup.refresh_statistics().map_err(|e| e.to_string())
        })
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

    /// True while someone is signed in (the demo starts signed in).
    #[must_use]
    pub fn signed_in(&self) -> bool {
        match self.demo_state() {
            Some(d) => d.auth.signed_in.is_some(),
            None => self.session().is_some(),
        }
    }

    /// The signed-in email (memory only; the demo's own when it runs).
    #[must_use]
    pub fn signed_in_email(&self) -> Option<String> {
        if let Some(d) = self.demo_state() {
            return d.auth.signed_in.clone();
        }
        self.sign_in_email
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Remember who signed in (P29 WP2).
    pub fn set_signed_in_email(&self, email: Option<String>) {
        *self
            .sign_in_email
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = email;
    }

    /// Automatic sign-in for this app data folder (Q-P29o).
    #[must_use]
    pub fn auto_sign_in(&self) -> cl_settings::AutoSignIn {
        let dir = self
            .store
            .path()
            .parent()
            .map_or_else(|| self.backup_dir.clone(), Path::to_path_buf);
        cl_settings::AutoSignIn::in_dir(&dir)
    }

    /// Turns automatic sign-in off (a password change, a recovery, a
    /// temporary password or a second user, Q-P29o). Best effort: a
    /// keychain refusal is logged without the value.
    pub fn clear_auto_sign_in(&self) {
        if let Err(e) = self.auto_sign_in().clear(self.secrets.as_ref()) {
            tracing::warn!(error = %e, "automatic sign-in could not be cleared from the keychain");
        }
    }

    /// True once Sign out ran: automatic sign-in waits for the next start.
    #[must_use]
    pub fn auto_sign_in_skipped(&self) -> bool {
        self.skip_auto_sign_in
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Skip automatic sign-in for the rest of this run.
    pub fn skip_auto_sign_in(&self) {
        self.skip_auto_sign_in
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Signs out (File › Sign Out, P29 WP2): the session ends on the engine
    /// (best effort, never aborted), the email is forgotten and automatic
    /// sign-in waits for the next start. The caller closes the company
    /// first.
    pub fn sign_out(&self) {
        self.skip_auto_sign_in();
        self.set_signed_in_email(None);
        self.reset_rights();
        if let Some(mut d) = self.demo_state() {
            d.auth.signed_in = None;
            d.open = None;
            return;
        }
        let session = self
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let (Some(session), Some(live)) = (session, self.live.as_ref())
            && let Ok(group) = live.queue.group()
        {
            let label = cl_api::OrphanLabel {
                what: "orphan.lifecycle",
                key: None,
            };
            let _ = group.write(&live.client, label, move |_c| async move {
                Ok(session.logout().await.is_ok())
            });
        }
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

    /// Messages waiting in the Outbox, as last read (P20).
    #[must_use]
    pub fn outbox_count(&self) -> Option<u32> {
        self.outbox.lock().map_or(None, |g| *g)
    }

    /// Notes how many messages wait in the Outbox.
    pub fn set_outbox_count(&self, n: Option<u32>) {
        if let Ok(mut g) = self.outbox.lock() {
            *g = n;
        }
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

    /// The signed-in user's rights in the open company (P29). In the demo,
    /// a non-owner role set on the demo (`demo.intake.role`) is not an
    /// Admin.
    #[must_use]
    pub fn rights(&self) -> cl_model::permissions::Rights {
        let r = self
            .rights
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match self.demo_state() {
            Some(d) if !cl_model::permissions::is_admin_role(&d.intake.role(), false) => {
                r.without_admin()
            }
            _ => r,
        }
    }

    /// Keeps new rights and tells the shell (`CompanyEvent::Rights`).
    pub fn set_rights(&self, r: cl_model::permissions::Rights) {
        *self.rights.lock().unwrap_or_else(PoisonError::into_inner) = r.clone();
        self.push(CompanyEvent::Rights(r));
    }

    /// The company just opened (None once closed).
    pub fn set_open_tenant(&self, t: Option<TenantId>) {
        *self
            .open_tenant
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = t;
    }

    /// The open company's tag rules (Q-180 required groups, Q-276 every
    /// line and the default tag), read from its local preferences each
    /// time, so a change in Preferences or the Tags window shows at the
    /// next form or save. The one place the forms' rules come from.
    #[must_use]
    pub fn tag_rules(&self) -> cl_model::lists::tags::TagRules {
        use cl_model::lists::tags::TagRules;
        if let Some(d) = self.demo_state() {
            return TagRules::from_prefs(
                d.require_tag_every_line,
                d.default_tag,
                &d.required_tag_groups,
            );
        }
        let open = *self
            .open_tenant
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        open.and_then(|t| {
            self.store.load().ok().and_then(|l| {
                l.get(t).map(|r| {
                    TagRules::from_prefs(
                        r.prefs.require_tag_every_line,
                        r.prefs.default_tag,
                        &r.prefs.required_tag_groups,
                    )
                })
            })
        })
        .unwrap_or_default()
    }

    /// No company open: back to everything (only Open Company works).
    pub fn reset_rights(&self) {
        self.set_open_tenant(None);
        *self.rights.lock().unwrap_or_else(PoisonError::into_inner) =
            cl_model::permissions::Rights::all();
    }

    /// Queue an event for the shell.
    pub fn push(&self, e: CompanyEvent) {
        // Every way of opening a company tells the shell with `Opened`:
        // the link learns which company's local rules apply from it.
        if let CompanyEvent::Opened { tenant_id, .. } = &e {
            self.set_open_tenant(Some(*tenant_id));
        }
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

/// The rights `/auth/me` reported (P29). The role comes from the engine's
/// row for the open company, else from the company list (`fallback_role`).
/// An answer without a permission map, or for another company, falls back
/// to the role alone (view-only at most unless Admin: fail closed).
#[must_use]
pub fn rights_from_me(
    me: &cl_api::Me,
    tenant: TenantId,
    fallback_role: &str,
) -> cl_model::permissions::Rights {
    use cl_model::permissions::Rights;
    let role = if me.role.is_empty() {
        fallback_role
    } else {
        me.role.as_str()
    };
    if me.active_tenant.is_some_and(|t| t != tenant) || me.permissions.is_empty() {
        return Rights::from_role_only(role, me.is_super_admin);
    }
    Rights::from_engine(role, me.is_super_admin, &me.permissions)
}

/// Reads the rights in the company just opened (`/auth/me`); on any error
/// the role alone decides (fail closed).
pub async fn read_rights(
    session: &Session,
    tenant: TenantId,
    fallback_role: &str,
) -> cl_model::permissions::Rights {
    match session.me().await {
        Ok(me) => rights_from_me(&me, tenant, fallback_role),
        Err(_) => cl_model::permissions::Rights::from_role_only(fallback_role, false),
    }
}

/// Reads the rights again after the engine refused something with 403
/// (P29 §5: rights may have changed mid-session). The shell polls it each
/// frame; one read at a time, and the rights change only when they differ.
#[derive(Default)]
pub struct RightsWatch {
    seen: u64,
    task: Option<(RequestGroup, Task<cl_api::Me>)>,
}

impl std::fmt::Debug for RightsWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RightsWatch")
            .field("seen", &self.seen)
            .field("reading", &self.task.is_some())
            .finish()
    }
}

impl RightsWatch {
    /// A company was opened: refusals before now belong to the last one.
    pub fn opened(&mut self, link: &CompanyLink) {
        self.seen = link.session().map_or(0, |s| s.forbidden_seen());
        self.task = None;
    }

    /// Starts a read when a new 403 came back; applies a finished read.
    /// Returns true while a read runs.
    pub fn poll(&mut self, link: &CompanyLink, tenant: Option<TenantId>, role: &str) -> bool {
        let (Some(session), Some(tenant)) = (link.session(), tenant) else {
            return false;
        };
        if let Some((_, task)) = self.task.as_mut() {
            let Some(r) = task.try_take() else {
                return true;
            };
            self.task = None;
            if let Ok(me) = r {
                let r = rights_from_me(&me, tenant, role);
                if r != link.rights() {
                    tracing::info!("the signed-in user's rights changed; menus follow");
                    link.set_rights(r);
                }
            }
            return false;
        }
        let n = session.forbidden_seen();
        if n <= self.seen {
            return false;
        }
        self.seen = n;
        let Some(Ok(group)) = link.group() else {
            return false;
        };
        if let Ok(p) = group.read(&session, |s| async move { s.me().await }) {
            self.task = Some((group, Task::Pending(p)));
        }
        self.task.is_some()
    }
}

/// Status-bar text for the signed-in user in a company (name and role; no
/// email). An owner or administrator reads "Admin"; the other engine roles
/// have plain words, and a role this build doesn't know is shown as its
/// words (A09 QA-9).
#[must_use]
pub fn user_text(display_name: &str, role: &str) -> String {
    let role = match role.trim() {
        "owner" | "super_admin" | "admin" => crate::i18n::t("status.role.admin"),
        "" => return display_name.to_string(),
        r if crate::i18n::has(&format!("status.role.{r}")) => {
            crate::i18n::t(&format!("status.role.{r}"))
        }
        other => {
            let words = other.replace('_', " ");
            let mut c = words.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase().collect::<String>() + c.as_str()
            })
        }
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

    /// A09 QA-9: a bookkeeper is not called "Admin".
    #[test]
    fn the_status_bar_names_each_role() {
        assert_eq!(user_text("Kurt", "admin"), "Kurt \u{b7} Admin");
        assert_eq!(user_text("bk", "bookkeeper"), "bk \u{b7} Bookkeeper");
        assert_eq!(user_text("acct", "accountant"), "acct \u{b7} Accountant");
        assert_eq!(user_text("v", "readonly"), "v \u{b7} View only");
        assert_eq!(user_text("x", "client_viewer"), "x \u{b7} Client viewer");
        assert_eq!(user_text("x", ""), "x");
    }
}
