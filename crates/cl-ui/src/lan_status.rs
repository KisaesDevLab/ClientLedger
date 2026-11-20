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
//! - live refresh: until the engine's change feed (U-294, `changes-v1`)
//!   exists, a 2 s ledger-version poll tells every window that
//!   transactions changed elsewhere (Q-P33i).
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
        matches!(self, LanState::HostDown | LanState::HostRestarting)
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
        // Unpaired, clock off, or gone: nothing reaches the host.
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

/// This computer's place on the office network now; `None` when it is
/// just this computer.
#[must_use]
pub fn state_of(link: &CompanyLink) -> Option<LanState> {
    if link.is_demo() {
        let d = link.demo_state()?;
        return match d.lan.mode {
            Mode::Off => None,
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
    /// The host stopped answering since the last time it did.
    was_covered: bool,
    /// "Connected again" shows until dismissed.
    back_note: bool,
    group: Option<(usize, RequestGroup)>,
    poll: Option<Task<LedgerStamp>>,
    last: Option<LedgerStamp>,
    next_poll: Option<Instant>,
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

    /// Reads the state for this frame and runs the refresh poll.
    pub fn tick(&mut self, link: Option<&Arc<CompanyLink>>, now: Instant) {
        let state = link.and_then(|l| state_of(l));
        WORKSTATION_NOW.store(state.is_some_and(LanState::workstation), Ordering::Relaxed);
        match state {
            Some(s) if s.covered() => self.was_covered = true,
            Some(LanState::Connected) if self.was_covered => {
                self.was_covered = false;
                self.back_note = true;
            }
            _ => {}
        }
        self.state = state;
        if let (Some(link), Some(s)) = (link, state) {
            self.refresh(link, s, now);
        } else {
            self.poll = None;
            self.last = None;
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
            match task.try_take() {
                None => return,
                Some(Ok(stamp)) => {
                    if self.last.is_some_and(|l| l != stamp) {
                        link.bus().publish(Change::Ledger(None));
                    }
                    self.last = Some(stamp);
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
            self.group = link.group().and_then(Result::ok).map(|g| (key, g));
        }
        let (Some(session), Some((_, group))) = (link.session(), self.group.as_ref()) else {
            self.next_poll = Some(now + POLL);
            return;
        };
        self.poll = group
            .read(&session, |s| async move { s.ledger_version().await })
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
        egui::Modal::new(egui::Id::unique("lan_host_down")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.label(RichText::new(t(state.status_key())).strong().size(16.0));
            ui.add_space(4.0);
            ui.label(t("lan.down.explain"));
            ui.add_space(4.0);
            ui.label(
                RichText::new(t_count("lan.down.kept", dirty, &[])).color(tokens.text_secondary),
            );
            ui.add_space(4.0);
            ui.label(RichText::new(t("lan.down.never_resent")).color(tokens.text_secondary));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t("lan.down.waiting"));
            });
            ui.horizontal(|ui| {
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

    /// For the control channel (no host address, no names).
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state.map(LanState::name),
            "covered": self.state.is_some_and(LanState::covered),
            "back_note": self.back_note,
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
        ] {
            assert!(crate::i18n::has(s.status_key()), "{s:?}");
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
