//! The office network as the main window shows it (P33 D4 steps 11–12):
//!
//! - the status bar's mode ("Sharing these books", "Connected to the host",
//!   "The host computer isn't answering", …);
//! - the host-down overlay: while a workstation can't reach the host, the
//!   app is covered and every open window stays as it was, unsaved edits
//!   included (kept in memory only, Q-P33n). Nothing is sent again on its
//!   own: a save that was under way when the link dropped is "not
//!   confirmed" in its form (the form's own check, which replays the same
//!   idempotency key, Q-P33al), and the owner saves the rest when ready;
//! - the maintenance and "update needed" banners;
//! - host-only actions (Q-P33y) disabled on workstations, with plain words;
//! - live refresh: every 2 s the engine's change feed (U-294,
//!   `changes-v1`) tells the windows which lists to read again (`reset`:
//!   all of them); an engine without it gets the ledger-version poll
//!   (Q-P33i, Q-P33an).
//!
//! The `--demo` office network drives it through `dev.lan_host_down`,
//! `dev.lan_host_up`, `dev.lan_maintenance` and `dev.lan_maintenance_off`.
//! Nothing here carries an amount, a name or a memo.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use cl_api::{LedgerStamp, RequestGroup};
use cl_lan::client::LinkState;
use cl_lan::settings::Mode;
use eframe::egui::{self, RichText};

use crate::company::demo::lan::DemoLink;
use crate::company::{CompanyLink, Task};
use crate::data_bus::Change;
use crate::i18n::{t, t_count};
use crate::lan_link::LanWorkstation;
use crate::theme::Tokens;

/// How often a LAN computer checks for others' changes (Q-P33j).
pub const POLL: Duration = Duration::from_secs(2);

/// What this computer is on the office network, and how its link is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanState {
    /// This computer shares its books.
    Host,
    /// Using the host's books; first answer not in yet.
    Connecting,
    /// Using the host's books.
    Connected,
    /// The host computer doesn't answer.
    HostDown,
    /// The host answers but its books are restarting.
    HostRestarting,
    /// The host's owner is restoring or importing.
    Maintenance,
    /// This computer and the host run different versions.
    UpdateNeeded,
    /// The host removed this computer (QA-11): it pairs again to go on.
    Unpaired,
}

impl LanState {
    /// A workstation (not the host).
    #[must_use]
    pub fn workstation(self) -> bool {
        self != LanState::Host
    }

    /// The app is covered until the host answers.
    #[must_use]
    pub fn covered(self) -> bool {
        matches!(
            self,
            LanState::HostDown | LanState::HostRestarting | LanState::Unpaired
        )
    }

    /// The status bar's words (i18n key).
    #[must_use]
    pub fn status_key(self) -> &'static str {
        match self {
            LanState::Host => "lan.status.sharing",
            LanState::Connecting => "lan.link.connecting",
            LanState::Connected => "lan.link.connected",
            LanState::HostDown => "lan.link.host-down",
            LanState::HostRestarting => "lan.link.host-engine-down",
            LanState::Maintenance => "lan.link.host-maintenance",
            LanState::UpdateNeeded => "lan.link.update-needed",
            LanState::Unpaired => "lan.link.unpaired",
        }
    }

    /// For the control channel.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            LanState::Host => "host",
            LanState::Connecting => "connecting",
            LanState::Connected => "connected",
            LanState::HostDown => "host_down",
            LanState::HostRestarting => "host_restarting",
            LanState::Maintenance => "maintenance",
            LanState::UpdateNeeded => "update_needed",
            LanState::Unpaired => "unpaired",
        }
    }
}

/// The workstation link state, as the main window shows it.
#[must_use]
pub fn of_link(s: &LinkState) -> LanState {
    match s {
        LinkState::Connecting => LanState::Connecting,
        LinkState::Connected { .. } => LanState::Connected,
        LinkState::HostEngineDown => LanState::HostRestarting,
        LinkState::HostMaintenance => LanState::Maintenance,
        LinkState::UpdateNeeded { .. } => LanState::UpdateNeeded,
        // QA-11: removed on the host; never "isn't answering".
        LinkState::Unpaired => LanState::Unpaired,
        // Clock off, or gone: nothing reaches the host.
        _ => LanState::HostDown,
    }
}

/// The workstation link running in this process (the app registers it
/// when it starts as a workstation).
static WORKSTATION: OnceLock<Mutex<Weak<LanWorkstation>>> = OnceLock::new();

/// Registers the running workstation link (weak: the app owns it).
pub fn set_workstation(w: &Arc<LanWorkstation>) {
    *WORKSTATION
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(w);
}

fn running_workstation() -> Option<Arc<LanWorkstation>> {
    WORKSTATION
        .get()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .upgrade()
}

/// The running workstation link's state as its `lan.link.` key suffix.
#[must_use]
pub fn workstation_link_key() -> Option<&'static str> {
    running_workstation().map(|w| w.state().key())
}

/// The status bar's one office-network line (QA-7, DESIGN.md window
/// anatomy: mode and server). A connected workstation names the host
/// ("Office network: connected to 192.168.1.20"); otherwise only the
/// host's message shows, in its precise words when the running link has
/// them (`link_key`, e.g. the clock being off). The host keeps "Sharing
/// these books with the office".
#[must_use]
pub fn status_text(state: LanState, link_key: Option<&str>, host: Option<&str>) -> String {
    match state {
        LanState::Host => t(state.status_key()),
        LanState::Connected => match host {
            Some(h) => crate::i18n::t_with("lan.status.workstation", &[("host", h)]),
            None => t("lan.status.workstation.unknown"),
        },
        _ => match link_key.filter(|k| *k != "connected") {
            Some(k) => t(&format!("lan.link.{k}")),
            None => t(state.status_key()),
        },
    }
}

/// The paired host's address as the status bar names it (no port).
#[must_use]
pub fn host_shown(link: &CompanyLink) -> Option<String> {
    if link.is_demo() {
        let d = link.demo_state()?;
        let a = d.lan.paired_host.clone()?;
        return Some(
            a.parse::<std::net::SocketAddr>()
                .map_or(a, |s| s.ip().to_string()),
        );
    }
    running_workstation().map(|w| w.host_addr().ip().to_string())
}

/// This computer's place on the office network now; `None` when it is
/// just this computer.
#[must_use]
pub fn state_of(link: &CompanyLink) -> Option<LanState> {
    if link.is_demo() {
        let d = link.demo_state()?;
        return match d.lan.mode {
            Mode::Off => None,
            // The status bar never says "Sharing" while sharing can't run
            // (the host certificate is missing).
            Mode::Host if d.lan.cert_missing => None,
            Mode::Host => Some(LanState::Host),
            Mode::Workstation => Some(match d.lan.link {
                DemoLink::Connected => LanState::Connected,
                DemoLink::HostDown => LanState::HostDown,
                DemoLink::Maintenance => LanState::Maintenance,
            }),
        };
    }
    if crate::lan_link::host_running() {
        return Some(LanState::Host);
    }
    running_workstation().map(|w| of_link(&w.state()))
}

/// Actions only the host computer may run in LAN mode (Q-P33y): restore,
/// back up, new and delete company, the imports (and their undo), payroll
/// import and post. The engine refuses them from a workstation anyway
/// (`HOST_ONLY`, U-288); the app says so first.
pub const HOST_ONLY: &[&str] = &[
    "file.restore",
    "file.backup",
    "file.full_backup",
    "file.new_company",
    "file.delete_company",
    "file.import_qbd",
    "file.import_system",
    "file.import_transactions",
    "payroll.import",
    // The transfer file is an engine archive written on the host
    // (`POST /tenants/{id}/archives`, host-only).
    "company.send_to_accountant",
    // Builds the practice company when it is missing (a new company).
    "file.open_practice",
];

static WORKSTATION_NOW: AtomicBool = AtomicBool::new(false);

/// `id` is host-only and this computer is a workstation now (menus,
/// shortcuts and the palette ask before running it).
#[must_use]
pub fn host_only_now(id: &str) -> bool {
    host_only(WORKSTATION_NOW.load(Ordering::Relaxed), id)
}

/// `id` is host-only and this computer is a `workstation`.
#[must_use]
pub fn host_only(workstation: bool, id: &str) -> bool {
    workstation && HOST_ONLY.contains(&id)
}

/// What a maintenance hold is for (PR-8 S6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// A restore replaces the books under everyone: the workstations are
    /// signed out (they open the restored company again).
    Restore,
    /// Payroll post and reverse, an import's commit and undo: the
    /// workstations stay signed in and only wait (503 `MAINTENANCE`) while
    /// it lasts, so their open windows and unsaved work are kept.
    Routine,
}

/// The engine routes that need the hold in LAN mode
/// (`requireMaintenanceInLan`, Q-P33y) and the cl-api calls this app makes
/// to them. Every call must run inside [`with_maintenance`]
/// (`every_maintenance_route_is_called_under_a_hold` checks both lists).
pub const MAINTENANCE_CALLS: &[(&str, &str)] = &[
    ("POST /tenant-archives/import", "cl_company::restore("),
    ("POST /imports/:id/commit", ".imports().commit("),
    (
        "POST /imports/batches/:id/commit",
        ".import_transactions().commit(",
    ),
    (
        "POST /imports/batches/:id/undo",
        ".import_transactions().undo(",
    ),
    ("POST /imports/batches/:id/undo", ".imports().undo("),
    ("POST /payroll-import/sessions/:id/post", ".payroll().post("),
    (
        "POST /payroll-import/sessions/:id/reverse",
        ".payroll().reverse(",
    ),
    // No caller in the app: the payroll check-register and checks posts.
    ("POST /payroll-import/sessions/:id/check-register/post", ""),
    ("POST /payroll-import/sessions/:id/checks/post", ""),
];

/// Runs `work` under maintenance when this computer is the LAN host (its
/// engine runs with LAN on, PR-8 S7: also when sharing itself could not
/// start) and the engine has `maintenance-v1` (U-293): restore, import
/// commit and undo, payroll post and reverse. The engine drains writes in
/// flight, ends the workstations' sessions for a [`Hold::Restore`] only,
/// then refuses them with 503 `MAINTENANCE` (never retried) until the hold
/// ends. The hold always ends, whatever `work` answered; a refused start
/// (`MAINTENANCE_BUSY` or `MAINTENANCE_HELD`) runs nothing.
///
/// # Errors
/// The start's refusal, or `work`'s own error.
pub async fn with_maintenance<T, E, F>(
    session: &cl_api::Session,
    hold: Hold,
    work: F,
) -> Result<T, E>
where
    E: From<cl_api::ApiError>,
    F: std::future::Future<Output = Result<T, E>>,
{
    let on = crate::lan_link::host_running() || session.client().lan_host_engine();
    if !on
        || !session
            .client()
            .capabilities()
            .has(cl_api::Capabilities::MAINTENANCE)
    {
        return work.await;
    }
    session.start_maintenance(hold == Hold::Routine).await?;
    // QA-9: the workstations' heartbeat says "maintenance" while it lasts.
    crate::lan_link::set_host_maintenance(true);
    // P34: scheduled backups wait while a hold is on.
    crate::backup_runner::set_hold(true);
    let r = work.await;
    if let Err(e) = session.end_maintenance().await {
        tracing::warn!(error = %e, "maintenance could not be ended; it ends when this sign-in does");
    }
    crate::lan_link::set_host_maintenance(false);
    crate::backup_runner::set_hold(false);
    r
}

/// What the host-down overlay did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    /// Not shown.
    None,
    /// Shown: nothing behind it takes input.
    Covered,
    /// The owner asked for the Office Network window (to fix the host's
    /// address or forget the host).
    OpenSetup,
}

/// The shell's office-network part: state, overlay, banners, refresh.
#[derive(Default)]
pub struct LanShell {
    state: Option<LanState>,
    /// The paired host's address (a workstation's status bar names it).
    host: Option<String>,
    /// The host stopped answering since the last time it did.
    was_covered: bool,
    /// "Connected again" shows until dismissed.
    back_note: bool,
    /// The host ended this computer's sign-in (QA-9: a restore or import
    /// on the host signs the workstations out): the banner says so and
    /// offers to sign in again, until a read works again.
    signed_out: bool,
    /// The host was in maintenance since the last read that worked.
    saw_maintenance: bool,
    /// "Sign in again" was clicked (the shell runs File › Sign Out, which
    /// closes the company, asking about unsaved windows, and shows the
    /// sign-in).
    sign_in_asked: bool,
    group: Option<(usize, RequestGroup)>,
    poll: Option<Task<Answer>>,
    last: Option<LedgerStamp>,
    /// The change feed's cursor (`changes-v1`); `None` before the first
    /// read, which answers `reset` and only primes it.
    cursor: Option<String>,
    next_poll: Option<Instant>,
}

/// One refresh read: the change feed when the host has it (U-294), else
/// the ledger-version stamp (Q-P33an fallback).
#[derive(Debug)]
enum Answer {
    Feed(cl_api::ChangePage),
    Ledger(LedgerStamp),
}

/// Every list a window may show: what a feed `reset` reloads.
fn everything() -> Vec<Change> {
    vec![
        Change::Ledger(None),
        Change::Contacts,
        Change::Accounts,
        Change::Tags,
        Change::Items,
        Change::Templates,
        Change::BillsAll,
        Change::BankFeed,
        Change::Company,
    ]
}

/// Every kind the engine's change feed writes (a `mybooks_feed` trigger,
/// engine migration 0289) and the notices it becomes (PR-8 S2). A kind
/// with no trigger has no row here
/// (`every_fed_kind_has_a_row_and_every_row_a_trigger`).
pub const FEED_KINDS: &[(&str, &[Change])] = &[
    // An edit, void or delete of any transaction: the registers, and any
    // vendor's duplicate-ref check (a bill is a transaction; PR-8 S1). The
    // Bank Feed reads again on any notice already (a remote accept is a
    // transaction).
    ("transactions", &[Change::Ledger(None), Change::BillsAll]),
    ("daily_sales_entries", &[Change::Ledger(None)]),
    ("contacts", &[Change::Contacts]),
    ("contact_tax_exemptions", &[Change::Contacts]),
    ("accounts", &[Change::Accounts]),
    ("tags", &[Change::Tags]),
    ("tag_groups", &[Change::Tags]),
    ("items", &[Change::Items]),
    ("unit_sets", &[Change::Items]),
    ("price_levels", &[Change::Items]),
    // Sales tax codes and agencies feed the same pickers as the tax items.
    ("sales_tax_items", &[Change::Items]),
    ("sales_tax_codes", &[Change::Items]),
    ("sales_tax_agencies", &[Change::Items]),
    ("je_templates", &[Change::Templates]),
    ("recurring_schedules", &[Change::Templates]),
    ("daily_sales_templates", &[Change::Templates]),
    ("invoice_templates", &[Change::Templates]),
    ("vendor_layout_templates", &[Change::Templates]),
    // Bank rules and documents waiting for review: Bank Feed and Review
    // Documents read again.
    ("conditional_rules", &[Change::BankFeed]),
    ("document_drafts", &[Change::BankFeed]),
    // The company row (settings, closing date) and the books' own row; the
    // role templates live with the company's people settings.
    ("companies", &[Change::Company]),
    ("tenants", &[Change::Company]),
    ("permission_templates", &[Change::Company]),
];

/// What the windows must read again for one feed page (ids and kinds
/// only). `reset` reloads everything; `upsert`, `delete` and `left`
/// all re-read the list the record is in, which drops a deleted or
/// no-longer-visible (`left`) record and shows an upserted one. Unknown
/// kinds are skipped. Each notice once.
#[must_use]
pub fn changes_of(page: &cl_api::ChangePage) -> Vec<Change> {
    if page.reset {
        return everything();
    }
    let mut out: Vec<Change> = Vec::new();
    for item in &page.changes {
        let Some((_, notices)) = FEED_KINDS.iter().find(|(k, _)| *k == item.kind) else {
            continue;
        };
        for change in *notices {
            if !out.contains(change) {
                out.push(change.clone());
            }
        }
    }
    out
}

impl std::fmt::Debug for LanShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanShell")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl LanShell {
    /// The state as of this frame.
    #[must_use]
    pub fn state(&self) -> Option<LanState> {
        self.state
    }

    /// The status bar's office-network line, if any (QA-7).
    #[must_use]
    pub fn status_line(&self) -> Option<String> {
        self.state.map(|s| {
            status_text(
                s,
                workstation_link_key().filter(|_| s.workstation()),
                self.host.as_deref(),
            )
        })
    }

    /// Reads the state for this frame and runs the refresh poll.
    pub fn tick(&mut self, link: Option<&Arc<CompanyLink>>, now: Instant) {
        let state = link.and_then(|l| state_of(l));
        self.host = match state {
            Some(s) if s.workstation() => link.and_then(|l| host_shown(l)),
            _ => None,
        };
        WORKSTATION_NOW.store(state.is_some_and(LanState::workstation), Ordering::Relaxed);
        match state {
            Some(s) if s.covered() => self.was_covered = true,
            Some(LanState::Connected) if self.was_covered => {
                self.was_covered = false;
                self.back_note = true;
            }
            _ => {}
        }
        if state == Some(LanState::Maintenance) {
            self.saw_maintenance = true;
        }
        self.state = state;
        if let (Some(link), Some(s)) = (link, state) {
            self.refresh(link, s, now);
        } else {
            self.poll = None;
            self.last = None;
            self.cursor = None;
        }
    }

    /// The ledger-version poll (live only; the demo ledger only changes in
    /// this app, whose saves reach the data bus already). A move tells
    /// every window that transactions changed somewhere.
    fn refresh(&mut self, link: &Arc<CompanyLink>, state: LanState, now: Instant) {
        if link.is_demo() || state.covered() {
            return;
        }
        if let Some(task) = self.poll.as_mut() {
            let answer = task.try_take();
            match &answer {
                None => return,
                Some(Ok(_)) => {
                    self.signed_out = false;
                    self.saw_maintenance = false;
                }
                Some(Err(cl_api::ApiError::Unauthorized(_))) if state.workstation() => {
                    if !self.signed_out {
                        tracing::info!("the host ended this computer's sign-in");
                    }
                    self.signed_out = true;
                }
                Some(Err(_)) => {}
            }
            match answer {
                None => return,
                Some(Ok(Answer::Ledger(stamp))) => {
                    if self.last.is_some_and(|l| l != stamp) {
                        link.bus().publish(Change::Ledger(None));
                    }
                    self.last = Some(stamp);
                }
                Some(Ok(Answer::Feed(page))) => {
                    // The first read primes the cursor and answers `reset`:
                    // the windows read again once, so a change committed
                    // between their own first reads and this one is not
                    // missed (PR-8 M3).
                    for change in changes_of(&page) {
                        link.bus().publish(change);
                    }
                    self.cursor = page.cursor.or_else(|| self.cursor.take());
                }
                Some(Err(e)) => {
                    tracing::debug!(error = %e, "ledger version not read");
                }
            }
            self.poll = None;
            self.next_poll = Some(now + POLL);
        }
        if self.next_poll.is_some_and(|n| now < n) {
            return;
        }
        // One request group per company link.
        let key = Arc::as_ptr(link) as usize;
        if self.group.as_ref().is_none_or(|(k, _)| *k != key) {
            self.last = None;
            self.cursor = None;
            self.group = link.group().and_then(Result::ok).map(|g| (key, g));
        }
        let (Some(session), Some((_, group))) = (link.session(), self.group.as_ref()) else {
            self.next_poll = Some(now + POLL);
            return;
        };
        let cursor = self.cursor.clone();
        self.poll = group
            .read(&session, move |s| async move {
                if s.client().capabilities().has(cl_api::Capabilities::CHANGES) {
                    s.changes(cursor.as_deref()).await.map(Answer::Feed)
                } else {
                    s.ledger_version().await.map(Answer::Ledger)
                }
            })
            .ok()
            .map(Task::Pending);
        if self.poll.is_none() {
            self.next_poll = Some(now + POLL);
        }
    }

    /// The demo office network's control-channel commands (any window).
    pub fn command(link: &CompanyLink, id: &str) -> bool {
        let Some(mut d) = link.demo_state() else {
            return false;
        };
        let l = &mut d.lan;
        let to = match id {
            "dev.lan_host_down" => DemoLink::HostDown,
            "dev.lan_host_up" | "dev.lan_maintenance_off" => DemoLink::Connected,
            "dev.lan_maintenance" => DemoLink::Maintenance,
            _ => return false,
        };
        // The scenarios start from "just this computer": the demo becomes
        // a workstation paired with its synthetic host first.
        if l.mode != Mode::Workstation {
            l.mode = Mode::Workstation;
            l.paired_host = Some("192.168.1.20:47615".into());
        }
        l.link = to;
        true
    }

    /// Back to "just this computer" (`dev.lan_demo_off`): the scenarios end
    /// where they began.
    pub fn demo_off(link: &CompanyLink) -> bool {
        let Some(mut d) = link.demo_state() else {
            return false;
        };
        d.lan.mode = Mode::Off;
        d.lan.paired_host = None;
        d.lan.link = DemoLink::Connected;
        true
    }

    /// The banner under the menu (maintenance, update needed, connected
    /// again). Draws nothing otherwise.
    pub fn banner(&mut self, ui: &mut egui::Ui, tokens: &Tokens, dirty: usize) {
        let text = match self.state {
            Some(LanState::Maintenance) => Some(t("lan.banner.maintenance")),
            Some(LanState::UpdateNeeded) => Some(t("lan.banner.update_needed")),
            Some(LanState::Connected) if self.signed_out => Some(t(if self.saw_maintenance {
                "lan.banner.signed_out.maintenance"
            } else {
                "lan.banner.signed_out"
            })),
            Some(LanState::Connected) if self.back_note => {
                Some(t_count("lan.banner.back", dirty, &[]))
            }
            _ => None,
        };
        let Some(text) = text else {
            return;
        };
        let warn = !matches!(self.state, Some(LanState::Connected));
        egui::Panel::top("lan_banner")
            .frame(
                egui::Frame::new()
                    .fill(if warn {
                        tokens.warn_surface
                    } else {
                        tokens.surface_panel
                    })
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let color = if warn {
                        tokens.warn_text
                    } else {
                        tokens.text_primary
                    };
                    ui.label(RichText::new(text).color(color));
                    if self.signed_out && self.state == Some(LanState::Connected) {
                        if ui
                            .button(t("lan.banner.sign_in"))
                            .on_hover_text(t("lan.banner.sign_in.tip"))
                            .clicked()
                        {
                            self.sign_in_asked = true;
                        }
                        return;
                    }
                    if self.back_note
                        && ui
                            .button(t("lan.banner.dismiss"))
                            .on_hover_text(t("lan.banner.dismiss.tip"))
                            .clicked()
                    {
                        self.back_note = false;
                    }
                });
            });
    }

    /// The host-down overlay: covers the app while the host doesn't
    /// answer. Not while the Office Network window is in front
    /// (`setup_in_front`): a host that moved to another address is fixed
    /// there, so a workstation is never locked out of its own settings.
    pub fn overlay(&mut self, ctx: &egui::Context, dirty: usize, setup_in_front: bool) -> Overlay {
        let Some(state) = self.state.filter(|s| s.covered()) else {
            return Overlay::None;
        };
        if setup_in_front {
            return Overlay::None;
        }
        let mut open_setup = false;
        let tokens = Tokens::of(ctx);
        // QA-12: never wider than the window (960 x 640 at 200% leaves 480
        // points); every line wraps.
        let width = (ctx.content_rect().width() - 48.0).clamp(200.0, 520.0);
        egui::Modal::new(egui::Id::unique("lan_host_down")).show(ctx, |ui| {
            ui.set_max_width(width);
            ui.label(RichText::new(t(state.status_key())).strong().size(16.0));
            ui.add_space(4.0);
            let unpaired = state == LanState::Unpaired;
            ui.label(t(if unpaired {
                "lan.down.unpaired"
            } else {
                "lan.down.explain"
            }));
            ui.add_space(4.0);
            ui.label(
                RichText::new(t_count("lan.down.kept", dirty, &[])).color(tokens.text_secondary),
            );
            ui.add_space(4.0);
            ui.label(RichText::new(t("lan.down.never_resent")).color(tokens.text_secondary));
            ui.add_space(8.0);
            if !unpaired {
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    ui.label(t("lan.down.waiting"));
                });
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(t("lan.down.check"))
                    .on_hover_text(t("lan.down.check.tip"))
                    .clicked()
                {
                    // The link checks every 2 s on its own; this only
                    // redraws at once.
                    ui.ctx().request_repaint();
                }
                if ui
                    .button(t("lan.down.settings"))
                    .on_hover_text(t("lan.down.settings.tip"))
                    .clicked()
                {
                    open_setup = true;
                }
            });
        });
        ctx.request_repaint_after(Duration::from_millis(500));
        if open_setup {
            Overlay::OpenSetup
        } else {
            Overlay::Covered
        }
    }

    /// "Sign in again" was clicked (taken once).
    pub fn take_sign_in(&mut self) -> bool {
        std::mem::take(&mut self.sign_in_asked)
    }

    /// For the control channel (no host address, no names).
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state.map(LanState::name),
            "covered": self.state.is_some_and(LanState::covered),
            "back_note": self.back_note,
            "signed_out": self.signed_out,
            "polling": self.poll.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> (tempfile::TempDir, Arc<CompanyLink>) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        (dir, link)
    }

    #[test]
    fn host_down_covers_then_connected_again_says_so_and_host_only_follows_the_mode() {
        let (_d, link) = demo();
        let mut s = LanShell::default();
        let now = Instant::now();
        s.tick(Some(&link), now);
        assert_eq!(s.state(), None);
        assert!(LanShell::command(&link, "dev.lan_host_down"));
        s.tick(Some(&link), now);
        assert_eq!(s.state(), Some(LanState::HostDown));
        assert_eq!(s.json()["covered"], true);
        let ws = s.state().is_some_and(LanState::workstation);
        assert!(host_only(ws, "file.restore"));
        assert!(!host_only(ws, "company.journal_entry"));
        assert!(!host_only(false, "file.restore"));
        assert!(LanShell::command(&link, "dev.lan_host_up"));
        s.tick(Some(&link), now);
        assert_eq!(s.state(), Some(LanState::Connected));
        assert_eq!(s.json()["back_note"], true);
        assert!(LanShell::command(&link, "dev.lan_maintenance"));
        s.tick(Some(&link), now);
        assert_eq!(s.json()["state"], "maintenance");
        assert!(LanShell::demo_off(&link));
        s.tick(Some(&link), now);
        assert_eq!(s.state(), None);
        assert!(!LanShell::command(&link, "dev.other"));
    }

    /// QA-7: one status-bar line per mode: a connected workstation names its
    /// host; down, only the host's message (the clock's precise words when
    /// the link has them); the host says it shares.
    #[test]
    fn the_status_line_names_the_host_and_only_the_host_message_when_down() {
        assert_eq!(
            status_text(LanState::Connected, Some("connected"), Some("192.168.1.20")),
            "Office network: connected to 192.168.1.20"
        );
        assert_eq!(
            status_text(LanState::Connected, None, None),
            "Office network: connected to the host"
        );
        assert_eq!(
            status_text(LanState::HostDown, None, Some("192.168.1.20")),
            t("lan.link.host-down")
        );
        assert_eq!(
            status_text(LanState::HostDown, Some("clock-skew"), None),
            t("lan.link.clock-skew")
        );
        assert_eq!(
            status_text(LanState::Host, None, None),
            "Sharing these books with the office"
        );
        // The demo's paired host is named without its port.
        let (_d, link) = demo();
        assert!(LanShell::command(&link, "dev.lan_host_up"));
        if let Some(mut d) = link.demo_state() {
            d.lan.paired_host = Some("192.168.1.20:47615".into());
        }
        let mut s = LanShell::default();
        s.tick(Some(&link), Instant::now());
        assert_eq!(
            s.status_line().as_deref(),
            Some("Office network: connected to 192.168.1.20")
        );
    }

    /// The Office Network window in front uncovers the app, so a host that
    /// moved can be fixed; otherwise the overlay covers it.
    #[test]
    fn host_down_overlay_steps_aside_for_the_office_network_window() {
        let (_d, link) = demo();
        let mut s = LanShell::default();
        assert!(LanShell::command(&link, "dev.lan_host_down"));
        s.tick(Some(&link), Instant::now());
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        let mut seen = Vec::new();
        for front in [false, true] {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                seen.push(s.overlay(ui.ctx(), 2, front));
            });
            out.textures_delta.clear();
        }
        assert_eq!(seen, vec![Overlay::Covered, Overlay::None]);
        assert!(crate::i18n::has("lan.down.settings"));
        assert!(crate::i18n::has("lan.ws.start_failed"));
        assert!(LanShell::demo_off(&link));
        s.tick(Some(&link), Instant::now());
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(s.overlay(ui.ctx(), 0, false), Overlay::None);
        });
        out.textures_delta.clear();
    }

    /// A workstation's company link (P33): live (not the demo), no
    /// supervisor, and with no link registered it is not on the office
    /// network yet (nothing covers the app; the status bar says why).
    #[test]
    fn a_workstation_company_link_has_no_supervisor() {
        let client = cl_api::Client::new(
            Arc::new(crate::lan_link::NoHost),
            cl_api::ClientConfig::default(),
        )
        .expect("client");
        let queue = cl_api::ApiQueue::start(Arc::new(|| {}), cl_api::QueueConfig::default())
            .expect("queue");
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::workstation(
            client,
            queue,
            cl_settings::RecentStore::in_dir(&dir.path().join("settings")),
            Arc::new(cl_settings::MemorySecrets::new()),
            dir.path().join("backups"),
        ));
        assert!(link.is_workstation());
        assert!(!link.is_demo());
        assert!(link.supervisor().is_none());
        assert_eq!(state_of(&link), None);
        let (_d, demo_link) = demo();
        assert!(!demo_link.is_workstation(), "the demo is never one");
        for id in ["company.send_to_accountant", "file.open_practice"] {
            assert!(host_only(true, id), "{id}");
        }
        assert!(!host_only(true, "file.export_all"));
    }

    #[test]
    fn feed_pages_map_to_the_lists_to_read_again() {
        use cl_api::{ChangeItem, ChangeOp, ChangePage};
        let item = |kind: &str, op| ChangeItem {
            kind: kind.into(),
            id: "x".into(),
            op,
        };
        let page = ChangePage {
            cursor: Some("2".into()),
            feed_epoch: Some("e".into()),
            reset: false,
            changes: vec![
                item("transactions", ChangeOp::Upsert),
                item("journal_lines", ChangeOp::Delete),
                item("contacts", ChangeOp::Upsert),
                item("accounts", ChangeOp::Left),
                item("tag_groups", ChangeOp::Upsert),
                item("something_new", ChangeOp::Upsert),
            ],
        };
        assert_eq!(
            changes_of(&page),
            vec![
                Change::Ledger(None),
                Change::BillsAll,
                Change::Contacts,
                Change::Accounts,
                Change::Tags
            ],
            "each list once; a hidden account (`left`) re-reads the chart; unknown kinds skipped"
        );
        // PR-8 S1, S2: a remote bill reaches every vendor panel; settings,
        // documents and sales tax codes reach their windows.
        let more = ChangePage {
            changes: vec![
                item("companies", ChangeOp::Upsert),
                item("document_drafts", ChangeOp::Upsert),
                item("sales_tax_codes", ChangeOp::Upsert),
                item("vendor_layout_templates", ChangeOp::Delete),
            ],
            ..ChangePage::default()
        };
        assert_eq!(
            changes_of(&more),
            vec![
                Change::Company,
                Change::BankFeed,
                Change::Items,
                Change::Templates
            ]
        );
        let bus = crate::data_bus::DataBus::new();
        let mut sub = bus.subscribe();
        for c in changes_of(&page) {
            bus.publish(c);
        }
        let vendor =
            cl_core::ids::ContactId::parse("00000000-0000-4000-8000-000000000101").expect("id");
        assert!(bus.take(&mut sub).bills_touch(vendor));
        let reset = ChangePage {
            reset: true,
            ..ChangePage::default()
        };
        assert_eq!(changes_of(&reset), everything());
        assert_eq!(changes_of(&ChangePage::default()).len(), 0);
    }

    /// PR-8 S6, S7: sharing did not start in this app, but the engine runs
    /// with LAN on, so the hold is still taken; a routine hold asks to keep
    /// the other sessions, a restore's does not; the hold always ends.
    #[tokio::test]
    async fn a_lan_engine_takes_the_hold_even_without_sharing() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/desktop/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "mode": "desktop", "initialized": true, "lan": true,
                "capabilities": [cl_api::Capabilities::MAINTENANCE]
            })))
            .mount(&server)
            .await;
        let hold = serde_json::json!({
            "active": true, "holderSession": "c", "holderUser": "u", "startedAt": "x"
        });
        Mock::given(method("POST"))
            .and(path("/api/v1/desktop/maintenance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(hold))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/desktop/maintenance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "active": false, "holderSession": null, "holderUser": null, "startedAt": null
            })))
            .mount(&server)
            .await;
        let s = cl_api::testing::stub_session(server.address().port());
        s.client().desktop_info().await.expect("desktop info");
        assert!(!crate::lan_link::host_running(), "no sharing in this test");
        let work = async { Ok::<u8, cl_api::ApiError>(7) };
        assert_eq!(
            with_maintenance(&s, Hold::Routine, work).await.ok(),
            Some(7)
        );
        let fails = async { Err::<u8, _>(cl_api::ApiError::Internal) };
        assert!(with_maintenance(&s, Hold::Restore, fails).await.is_err());
        let seen: Vec<String> = server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .filter(|r| r.url.path() == "/api/v1/desktop/maintenance")
            .map(|r| format!("{} {}", r.method, String::from_utf8_lossy(&r.body)))
            .collect();
        assert_eq!(
            seen,
            vec![
                r#"POST {"keepSessions":true}"#.to_string(),
                "DELETE ".to_string(),
                "POST {}".to_string(),
                "DELETE ".to_string(),
            ]
        );
    }

    /// PR-8 S2: every kind with a `mybooks_feed` trigger in the engine's
    /// migrations has a row in [`FEED_KINDS`], and every row a trigger (no
    /// dead rows that look like coverage).
    #[test]
    fn every_fed_kind_has_a_row_and_every_row_a_trigger() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../engine/packages/api/src/db/migrations");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("SKIPPED: no engine migrations at {}", dir.display());
            return;
        };
        let mut fed = std::collections::BTreeSet::new();
        let mut dropped = std::collections::BTreeSet::new();
        for e in entries.flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            let rollback = e.file_name().to_string_lossy().contains("rollback");
            for line in text.lines() {
                // `CREATE TRIGGER mybooks_feed ... ON <table> FOR EACH ROW`
                // and `DROP TRIGGER IF EXISTS mybooks_feed ON <table>;`
                if !line.contains("TRIGGER") || !line.contains("mybooks_feed ") {
                    continue;
                }
                let Some((_, rest)) = line.split_once(" ON ") else {
                    continue;
                };
                let table = rest
                    .split(|c: char| c.is_whitespace() || c == ';')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if line.trim_start().starts_with("CREATE") && !rollback {
                    fed.insert(table);
                } else if line.trim_start().starts_with("DROP") && !rollback {
                    dropped.insert(table);
                }
            }
        }
        assert!(!fed.is_empty(), "the feed's triggers were found");
        let fed: std::collections::BTreeSet<String> = fed.difference(&dropped).cloned().collect();
        let rows: std::collections::BTreeSet<String> =
            FEED_KINDS.iter().map(|(k, _)| (*k).to_string()).collect();
        assert_eq!(rows.len(), FEED_KINDS.len(), "a kind is listed twice");
        assert_eq!(rows, fed, "FEED_KINDS against the engine's feed triggers");
        assert!(FEED_KINDS.iter().all(|(_, n)| !n.is_empty()));
    }

    /// PR-8 B2: every engine route that needs the hold in LAN mode is in
    /// [`MAINTENANCE_CALLS`], and every call this app makes to one runs
    /// inside [`with_maintenance`] (a call outside it is refused on a
    /// sharing host with `MAINTENANCE_REQUIRED`, every time).
    #[test]
    fn every_maintenance_route_is_called_under_a_hold() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let routes = root.join("../../engine/packages/api/src/routes");
        if let Ok(dir) = std::fs::read_dir(&routes) {
            let mut found = std::collections::BTreeSet::new();
            for f in dir.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                let Some(prefix) = name.strip_suffix(".routes.ts") else {
                    continue;
                };
                let text = std::fs::read_to_string(f.path()).unwrap_or_default();
                for line in text
                    .lines()
                    .filter(|l| l.contains("requireMaintenanceInLan()"))
                {
                    // `xRouter.post('/path', requireMaintenanceInLan(), ...`
                    let Some((head, rest)) = line.split_once("('") else {
                        continue;
                    };
                    let Some(verb) = head.rsplit('.').next() else {
                        continue;
                    };
                    let Some((p, _)) = rest.split_once('\'') else {
                        continue;
                    };
                    found.insert(format!("{} /{prefix}{p}", verb.to_uppercase()));
                }
            }
            let listed: std::collections::BTreeSet<String> = MAINTENANCE_CALLS
                .iter()
                .map(|(r, _)| (*r).to_string())
                .collect();
            assert_eq!(found, listed, "the engine's maintenance routes changed");
        } else {
            eprintln!(
                "SKIPPED (route list): no engine sources at {}",
                routes.display()
            );
        }
        // Every call in the app's sources sits inside `with_maintenance(`.
        let mut files = Vec::new();
        let mut dirs = vec![root.join("src")];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).expect("src").flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("lan_status.rs")
                {
                    files.push(p);
                }
            }
        }
        let mut calls = 0;
        for f in &files {
            let text = std::fs::read_to_string(f).expect("read");
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                for (route, needle) in MAINTENANCE_CALLS {
                    if needle.is_empty() || !line.contains(needle) {
                        continue;
                    }
                    calls += 1;
                    let from = i.saturating_sub(12);
                    assert!(
                        lines[from..=i]
                            .iter()
                            .any(|l| l.contains("with_maintenance(")),
                        "{}:{}: {route} called outside with_maintenance",
                        f.display(),
                        i + 1
                    );
                }
            }
        }
        assert!(calls >= 7, "the call sites were found ({calls})");
    }

    #[test]
    fn link_states_map_and_every_key_has_words() {
        assert_eq!(of_link(&LinkState::HostDown), LanState::HostDown);
        assert_eq!(of_link(&LinkState::HostMaintenance), LanState::Maintenance);
        assert_eq!(
            of_link(&LinkState::HostEngineDown),
            LanState::HostRestarting
        );
        for s in [
            LanState::Host,
            LanState::Connecting,
            LanState::Connected,
            LanState::HostDown,
            LanState::HostRestarting,
            LanState::Maintenance,
            LanState::UpdateNeeded,
            LanState::Unpaired,
        ] {
            assert!(crate::i18n::has(s.status_key()), "{s:?}");
        }
        // Every link state has words (the Office Network window and the
        // workstation's status bar show them).
        for s in [
            LinkState::Connecting,
            LinkState::HostDown,
            LinkState::HostEngineDown,
            LinkState::HostMaintenance,
            LinkState::Unpaired,
        ] {
            assert!(crate::i18n::has(&format!("lan.link.{}", s.key())), "{s:?}");
        }
        for k in [
            "lan.banner.maintenance",
            "lan.banner.update_needed",
            "lan.banner.back.one",
            "lan.banner.dismiss",
            "lan.down.explain",
            "lan.down.kept.one",
            "lan.down.never_resent",
            "lan.down.check",
            "lan.host_only",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
        // One entry per key in the office network's words.
        let mut keys: Vec<&str> = crate::i18n::lan::ENGLISH.iter().map(|(k, _)| *k).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "a key is listed twice");
        for id in HOST_ONLY {
            assert!(cl_model::commands::find(id).is_some(), "{id}");
        }
    }
}
