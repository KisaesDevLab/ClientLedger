//! Office Network (Multi-User) (P33 D4 step 10, Q-108–Q-111): share this
//! computer's books (host) or use another computer's (workstation).
//!
//! No mockup exists for this window (Q-P33aj): it follows DESIGN.md's
//! settings-window pattern (a heading, a purpose line, cards).
//!
//! - **Host:** the network connection (private ranges only, Q-P33c; the
//!   others are listed with the reason), the security fingerprint, the
//!   pairing code and "Allow this computer?" (Q-P33b), the paired computers
//!   with Remove, "let others find this one" (mDNS, Q-P33d), keep sharing
//!   and start at sign-in (Q-P33f), and firewall guidance only (Q-P33e).
//! - **Workstation:** hosts found on the network or a typed address, the
//!   code from the host, then each person signs in as usual.
//!
//! The window talks to a [`LanControl`]: the `--demo` office network
//! (synthetic, no sockets) or the live one ([`crate::lan_link::LiveLan`]).
//! The pairing code and the fingerprint are the only secrets-looking values
//! shown, and both are meant to be shown; neither goes into `state()` or a
//! log.

use std::sync::Arc;

use cl_api::LanAuditEvent;
use cl_core::CalendarDate;
use cl_lan::policy::Candidate;
use cl_lan::settings::Mode;
use cl_model::CommandId;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::demo::lan::DemoLink;
use crate::company::{CompanyLink, spawn_lifecycle};
use crate::i18n::{t, t_count, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "company.lan_setup";

/// The default port (shown in the firewall guidance).
const PORT: u16 = cl_lan::host::DEFAULT_PORT;

/// The window for `kind`, if it is this one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(cl_core::ids::TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(LanSetupScreen::new(Arc::clone(link))) as _)
}

/// The pairing code as the host shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeShown {
    /// No code.
    None,
    /// Showing (`display`), for `minutes` more, `tries` wrong tries left.
    Active {
        /// `XXXXX-XXXXX-XXXXX-XXXXX`.
        display: String,
        /// Minutes left (rounded up).
        minutes: u64,
        /// Wrong tries left.
        tries: u8,
    },
    /// Too many wrong tries: from how many computers, and the address of
    /// the last one (shown so the owner can tell a typo from a stranger).
    Burned {
        /// Computers that tried.
        computers: usize,
        /// The last wrong try's address.
        last_peer: Option<String>,
    },
}

/// One paired computer as the window lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRow {
    /// Its name.
    pub label: String,
    /// Fingerprint (hex), for Remove.
    pub fingerprint: String,
    /// When it was paired, as shown.
    pub paired: String,
    /// People signed in from it (when known).
    pub signed_in: Option<u32>,
}

/// Everything the window shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanView {
    /// What this computer is now.
    pub mode: Mode,
    /// The network connections, judged.
    pub interfaces: Vec<Candidate>,
    /// The one chosen for sharing.
    pub chosen: Option<usize>,
    /// The host fingerprint (hex), once sharing.
    pub fingerprint: Option<String>,
    /// The pairing code.
    pub code: CodeShown,
    /// A computer waiting for Allow: its name and address.
    pub request: Option<(String, String)>,
    /// Paired computers.
    pub devices: Vec<DeviceRow>,
    /// Let other computers find this one.
    pub announce: bool,
    /// Keep sharing in the background and start at sign-in.
    pub start_at_sign_in: bool,
    /// Hosts found (name, address).
    pub found: Vec<(String, String)>,
    /// The host this computer is paired with.
    pub paired_host: Option<String>,
    /// The workstation link (i18n key under `lan.link.`).
    pub link: &'static str,
    /// The change waits for a restart.
    pub restart_needed: bool,
    /// Why sharing could not start, when the owner can fix it here (an
    /// i18n key; review M2: the host certificate file is missing).
    pub host_problem: Option<&'static str>,
}

/// The office network the window drives.
pub trait LanControl: Send {
    /// What to show now.
    fn view(&self) -> LanView;
    /// Starts sharing on connection `chosen` (true), or stops (false).
    ///
    /// # Errors
    /// An i18n key.
    fn set_host(&mut self, on: bool, chosen: Option<usize>) -> Result<(), &'static str>;
    /// Shows a new pairing code.
    fn new_code(&mut self);
    /// Stops pairing.
    fn cancel_code(&mut self);
    /// Allow or Don't allow the waiting computer.
    fn decide(&mut self, allow: bool);
    /// Removes paired computer `i` (and ends its sign-ins).
    fn remove(&mut self, i: usize);
    /// Makes a new host certificate: every paired computer is removed
    /// (sign-ins ended) and pairs again (review M2, S7).
    fn regenerate(&mut self);
    /// Let other computers find this one.
    fn set_announce(&mut self, on: bool);
    /// Keep sharing after closing and start at sign-in.
    ///
    /// # Errors
    /// An i18n key.
    fn set_start_at_sign_in(&mut self, on: bool) -> Result<(), &'static str>;
    /// Looks for hosts again.
    fn search(&mut self);
    /// Starts pairing with `addr` using `code`.
    fn pair(&mut self, addr: &str, code: &str);
    /// The pairing's answer, once.
    fn poll_pair(&mut self) -> Option<Result<(), &'static str>>;
    /// Pairing is under way (waiting for Allow on the host).
    fn pairing(&self) -> bool;
    /// Stops using the paired host.
    fn forget_host(&mut self);
    /// Audit events to send (taken once).
    fn take_events(&mut self) -> Vec<(LanAuditEvent, Option<String>, Option<String>)>;
}

/// The `--demo` office network.
struct DemoControl {
    link: Arc<CompanyLink>,
    answer: Option<Result<(), &'static str>>,
}

impl DemoControl {
    fn with<R>(&self, f: impl FnOnce(&mut crate::company::demo::lan::DemoLan) -> R) -> Option<R> {
        self.link.demo_state().map(|mut d| f(&mut d.lan))
    }
}

impl LanControl for DemoControl {
    fn view(&self) -> LanView {
        self.with(|l| LanView {
            mode: l.mode,
            interfaces: l.interfaces.clone(),
            chosen: l.chosen,
            fingerprint: (l.mode == Mode::Host && !l.cert_missing)
                .then(|| crate::company::demo::lan::FINGERPRINT.to_string()),
            code: if l.code_burned {
                CodeShown::Burned {
                    computers: 2,
                    last_peer: Some(crate::company::demo::lan::BURN_PEER.to_string()),
                }
            } else if l.code_on {
                CodeShown::Active {
                    display: crate::company::demo::lan::CODE.to_string(),
                    minutes: 10,
                    tries: 5,
                }
            } else {
                CodeShown::None
            },
            request: l
                .request
                .as_ref()
                .map(|r| (r.label.clone(), r.peer.clone())),
            devices: l
                .devices
                .iter()
                .map(|d| DeviceRow {
                    label: d.label.clone(),
                    fingerprint: d.fingerprint.clone(),
                    paired: d.paired.clone(),
                    signed_in: Some(d.signed_in),
                })
                .collect(),
            announce: l.announce,
            start_at_sign_in: l.start_at_sign_in,
            found: l.found.clone(),
            paired_host: l.paired_host.clone(),
            link: match l.link {
                DemoLink::Connected => "connected",
                DemoLink::HostDown => "host-down",
                DemoLink::Maintenance => "host-maintenance",
            },
            restart_needed: false,
            host_problem: l.cert_missing.then_some("lan.host.cert_missing"),
        })
        .unwrap_or_else(|| LanView {
            mode: Mode::Off,
            interfaces: Vec::new(),
            chosen: None,
            fingerprint: None,
            code: CodeShown::None,
            request: None,
            devices: Vec::new(),
            announce: false,
            start_at_sign_in: false,
            found: Vec::new(),
            paired_host: None,
            link: "connecting",
            restart_needed: false,
            host_problem: None,
        })
    }

    fn set_host(&mut self, on: bool, chosen: Option<usize>) -> Result<(), &'static str> {
        self.with(|l| {
            if on {
                let ok = chosen
                    .and_then(|i| l.interfaces.get(i))
                    .is_some_and(|c| c.refusal.is_none());
                if !ok {
                    return Err("lan.host.iface.none");
                }
                l.chosen = chosen;
            }
            l.set_host(on);
            Ok(())
        })
        .unwrap_or(Err("lan.host.iface.none"))
    }

    fn new_code(&mut self) {
        self.with(|l| {
            l.code_on = true;
            l.code_burned = false;
        });
    }

    fn cancel_code(&mut self) {
        self.with(|l| {
            l.code_on = false;
            l.code_burned = false;
            l.request = None;
        });
    }

    fn decide(&mut self, allow: bool) {
        self.with(|l| l.decide(allow));
    }

    fn remove(&mut self, i: usize) {
        self.with(|l| l.remove(i));
    }

    fn regenerate(&mut self) {
        self.with(crate::company::demo::lan::DemoLan::regenerate);
    }

    fn set_announce(&mut self, on: bool) {
        self.with(|l| l.announce = on);
    }

    fn set_start_at_sign_in(&mut self, on: bool) -> Result<(), &'static str> {
        self.with(|l| l.start_at_sign_in = on);
        Ok(())
    }

    fn search(&mut self) {}

    fn pair(&mut self, addr: &str, code: &str) {
        self.answer = self.with(|l| l.pair(addr, code));
    }

    fn poll_pair(&mut self) -> Option<Result<(), &'static str>> {
        self.answer.take()
    }

    fn pairing(&self) -> bool {
        false
    }

    fn forget_host(&mut self) {
        self.with(|l| {
            l.paired_host = None;
            l.mode = Mode::Off;
        });
    }

    fn take_events(&mut self) -> Vec<(LanAuditEvent, Option<String>, Option<String>)> {
        Vec::new()
    }
}

/// The Office Network window.
pub struct LanSetupScreen {
    link: Arc<CompanyLink>,
    control: Box<dyn LanControl>,
    /// The mode the owner picked (the window shows its setup before it is
    /// on).
    choice: Mode,
    /// The connection picked for sharing.
    pick: Option<usize>,
    addr: String,
    code: String,
    error: Option<&'static str>,
    paired_note: bool,
    /// The events being sent (sign-ins ended, audit rows).
    sending: Option<crate::company::Task<Result<(), &'static str>>>,
}

impl LanSetupScreen {
    /// The window over the demo or the live office network.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> LanSetupScreen {
        let control: Box<dyn LanControl> = if link.is_demo() {
            Box::new(DemoControl {
                link: Arc::clone(&link),
                answer: None,
            })
        } else {
            Box::new(crate::lan_link::LiveLan::open())
        };
        LanSetupScreen::with_control(link, control)
    }

    /// The window over `control` (tests).
    #[must_use]
    pub fn with_control(link: Arc<CompanyLink>, control: Box<dyn LanControl>) -> LanSetupScreen {
        let v = control.view();
        let pick = v
            .chosen
            .or_else(|| v.interfaces.iter().position(|c| c.refusal.is_none()));
        LanSetupScreen {
            link,
            choice: v.mode,
            pick,
            addr: v.paired_host.clone().unwrap_or_default(),
            code: String::new(),
            error: None,
            paired_note: false,
            sending: None,
            control,
        }
    }

    /// Sends the control's events: a removed computer's sign-ins end on
    /// the engine (review S7), then the audit rows. A computer whose
    /// sign-ins could not be ended is reported in the window.
    fn send_events(&mut self) {
        if let Some(task) = self.sending.as_mut()
            && let Some(r) = task.try_take()
        {
            self.sending = None;
            if let Ok(Err(k)) = r {
                self.error = Some(k);
            }
        }
        let events = self.control.take_events();
        if events.is_empty() || self.link.is_demo() {
            return;
        }
        let Some(client) = self.link.client().cloned() else {
            return;
        };
        let session = self.link.session();
        let Some(Ok(group)) = self.link.group() else {
            self.error = Some("lan.remove.sessions_failed");
            return;
        };
        match spawn_lifecycle(&self.link, &group, move || {
            crate::lan_link::record_events(client, session, events)
        }) {
            Ok(task) => self.sending = Some(task),
            Err(_) => self.error = Some("lan.remove.sessions_failed"),
        }
    }

    fn host_card(&mut self, ui: &mut Ui, tokens: &Tokens, v: &LanView) {
        let sharing = v.mode == Mode::Host;
        if let Some(k) = v.host_problem {
            // Review M2: plain words and the way out, never a silent new
            // certificate.
            form::card(ui, tokens, |ui| {
                form::inline_warning(ui, tokens, &t(k));
                if ui
                    .button(t("lan.host.regenerate"))
                    .on_hover_text(t("lan.host.regenerate.tip"))
                    .clicked()
                {
                    self.control.regenerate();
                }
                // Sharing is not running: nothing below (connection,
                // pairing code) would work, so only the way out shows.
                if sharing && ui.button(t("lan.host.off")).clicked() {
                    self.error = self.control.set_host(false, None).err();
                }
            });
            return;
        }
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("lan.host.iface")).strong());
            form::hint(ui, tokens, &t("lan.host.iface_hint"));
            if !v.interfaces.iter().any(|c| c.refusal.is_none()) {
                form::inline_warning(ui, tokens, &t("lan.host.iface.none"));
            }
            ui.add_enabled_ui(!sharing, |ui| {
                for (i, c) in v.interfaces.iter().enumerate() {
                    let text = format!("{} ({})", c.addr.name, c.addr.ip);
                    match c.refusal {
                        None => {
                            if ui.radio(self.pick == Some(i), text).clicked() {
                                self.pick = Some(i);
                            }
                        }
                        Some(r) => {
                            let why = t(&format!("lan.refusal.{}", r.key()));
                            ui.add_enabled(false, egui::RadioButton::new(false, text))
                                .on_disabled_hover_text(t_with(
                                    "lan.host.iface.refused",
                                    &[
                                        ("name", &c.addr.name),
                                        ("ip", &c.addr.ip.to_string()),
                                        ("why", &why),
                                    ],
                                ));
                        }
                    }
                }
            });
            ui.add_space(6.0);
            if sharing {
                if let Some(c) = v.chosen.and_then(|i| v.interfaces.get(i)) {
                    form::inline_note(
                        ui,
                        tokens,
                        &t_with(
                            "lan.host.sharing",
                            &[
                                ("name", &c.addr.name),
                                ("ip", &c.addr.ip.to_string()),
                                ("port", &PORT.to_string()),
                            ],
                        ),
                    );
                }
                form::hint(ui, tokens, &t("lan.host.autologin"));
                if ui.button(t("lan.host.off")).clicked() {
                    self.error = self.control.set_host(false, None).err();
                }
            } else {
                let can = self
                    .pick
                    .and_then(|i| v.interfaces.get(i))
                    .is_some_and(|c| c.refusal.is_none());
                ui.add_enabled_ui(can, |ui| {
                    if crate::widgets::primary_button(ui, &t("lan.host.on")).clicked() {
                        self.error = self.control.set_host(true, self.pick).err();
                    }
                });
            }
        });
        if !sharing {
            return;
        }
        ui.add_space(6.0);
        form::card(ui, tokens, |ui| {
            if let Some(fp) = &v.fingerprint {
                ui.label(RichText::new(t("lan.host.fingerprint")).strong())
                    .on_hover_text(t("lan.host.fingerprint.tip"));
                ui.label(RichText::new(group_hex(fp)).monospace())
                    .on_hover_text(t("lan.host.fingerprint.tip"));
                ui.add_space(6.0);
            }
            ui.label(RichText::new(t("lan.host.code")).strong());
            form::hint(ui, tokens, &t("lan.host.code.explain"));
            match &v.code {
                CodeShown::None => {
                    if ui.button(t("lan.host.code.new")).clicked() {
                        self.control.new_code();
                    }
                }
                CodeShown::Active {
                    display,
                    minutes,
                    tries,
                } => {
                    ui.label(RichText::new(display).monospace().size(20.0));
                    form::hint(
                        ui,
                        tokens,
                        &t_with(
                            "lan.host.code.left",
                            &[("min", &minutes.to_string()), ("tries", &tries.to_string())],
                        ),
                    );
                    if ui.button(t("lan.host.code.cancel")).clicked() {
                        self.control.cancel_code();
                    }
                }
                CodeShown::Burned {
                    computers,
                    last_peer,
                } => {
                    form::inline_warning(ui, tokens, &t("lan.host.code.burned"));
                    let from = match last_peer {
                        Some(peer) => {
                            t_count("lan.host.code.burned_from", *computers, &[("peer", peer)])
                        }
                        None => t_count("lan.host.code.burned_by", *computers, &[]),
                    };
                    form::hint(ui, tokens, &from);
                    if ui.button(t("lan.host.code.new")).clicked() {
                        self.control.new_code();
                    }
                }
            }
            if let Some((label, peer)) = &v.request {
                ui.add_space(6.0);
                form::inline_warning(
                    ui,
                    tokens,
                    &t_with("lan.host.request", &[("label", label), ("peer", peer)]),
                );
                ui.horizontal(|ui| {
                    if crate::widgets::primary_button(ui, &t("lan.host.allow")).clicked() {
                        self.control.decide(true);
                    }
                    if ui.button(t("lan.host.deny")).clicked() {
                        self.control.decide(false);
                    }
                });
            }
        });
        ui.add_space(6.0);
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("lan.host.devices")).strong());
            if v.devices.is_empty() {
                form::hint(ui, tokens, &t("lan.host.devices.none"));
            }
            let mut remove = None;
            egui::Grid::new("lan_devices")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    for (i, d) in v.devices.iter().enumerate() {
                        ui.label(&d.label);
                        ui.label(
                            RichText::new(t_with("lan.host.paired_on", &[("date", &d.paired)]))
                                .color(tokens.text_secondary),
                        );
                        ui.label(d.signed_in.map_or_else(String::new, |n| {
                            t_count(
                                "lan.host.signed_in",
                                usize::try_from(n).unwrap_or(usize::MAX),
                                &[],
                            )
                        }));
                        if ui
                            .button(t("lan.host.remove"))
                            .on_hover_text(t("lan.host.remove.tip"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            if let Some(i) = remove {
                self.control.remove(i);
            }
            ui.add_space(6.0);
            let mut announce = v.announce;
            if ui.checkbox(&mut announce, t("lan.host.announce")).changed() {
                self.control.set_announce(announce);
            }
            let mut start = v.start_at_sign_in;
            if ui
                .checkbox(&mut start, t("lan.host.start_at_sign_in"))
                .on_hover_text(t("lan.host.start_at_sign_in.tip"))
                .changed()
            {
                self.error = self.control.set_start_at_sign_in(start).err();
            }
            ui.add_space(6.0);
            form::hint(
                ui,
                tokens,
                &t_with("lan.host.firewall", &[("port", &PORT.to_string())]),
            );
        });
    }

    fn workstation_card(&mut self, ui: &mut Ui, tokens: &Tokens, v: &LanView) {
        form::card(ui, tokens, |ui| {
            if let Some(host) = &v.paired_host
                && v.mode == Mode::Workstation
            {
                form::inline_note(ui, tokens, &t_with("lan.ws.paired", &[("addr", host)]));
                ui.label(t(&format!("lan.link.{}", v.link)));
                if ui.button(t("lan.ws.forget")).clicked() {
                    self.control.forget_host();
                    self.choice = Mode::Off;
                }
                return;
            }
            ui.label(RichText::new(t("lan.ws.found")).strong());
            if v.found.is_empty() {
                form::hint(ui, tokens, &t("lan.ws.none_found"));
            }
            for (name, addr) in &v.found {
                ui.horizontal(|ui| {
                    ui.label(format!("{name} \u{2014} {addr}"));
                    if ui.button(t("lan.ws.use")).clicked() {
                        self.addr.clone_from(addr);
                    }
                });
            }
            if ui.button(t("lan.ws.search")).clicked() {
                self.control.search();
            }
            ui.add_space(6.0);
            form::field(ui, &t("lan.ws.address"), &mut self.addr);
            form::hint(ui, tokens, &t("lan.ws.address_hint"));
            form::field(ui, &t("lan.ws.code"), &mut self.code);
            ui.add_space(6.0);
            if self.control.pairing() {
                form::progress_line(ui, tokens, &t("lan.ws.waiting"));
            } else if crate::widgets::primary_button(ui, &t("lan.ws.pair")).clicked() {
                self.error = None;
                let (addr, code) = (self.addr.clone(), self.code.clone());
                self.control.pair(&addr, &code);
            }
        });
    }

    fn poll(&mut self) {
        if let Some(r) = self.control.poll_pair() {
            match r {
                Ok(()) => {
                    self.code.clear();
                    self.paired_note = true;
                    self.error = None;
                }
                Err(k) => self.error = Some(k),
            }
        }
        self.send_events();
    }
}

/// `3f9a 1c0e 7b2d …`: the fingerprint in groups of four, for reading
/// aloud.
#[must_use]
pub fn group_hex(hex: &str) -> String {
    hex.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

impl Screen for LanSetupScreen {
    fn title(&self) -> String {
        t("cmd.company.lan_setup")
    }

    fn can_save(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        if self.control.pairing() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        let tokens = Tokens::of(ui.ctx());
        let v = self.control.view();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                form::heading(ui, &tokens, &t("cmd.company.lan_setup"));
                form::hint(ui, &tokens, &t("lan.purpose"));
                ui.add_space(6.0);
                form::card(ui, &tokens, |ui| {
                    ui.label(RichText::new(t("lan.mode")).strong());
                    let busy = v.mode != Mode::Off;
                    for (m, key) in [
                        (Mode::Off, "lan.mode.off"),
                        (Mode::Host, "lan.mode.host"),
                        (Mode::Workstation, "lan.mode.workstation"),
                    ] {
                        // While sharing or paired, the owner stops that
                        // first (Stop sharing / Stop using this host).
                        let enabled = !busy || m == v.mode;
                        if ui
                            .add_enabled(enabled, egui::RadioButton::new(self.choice == m, t(key)))
                            .clicked()
                        {
                            self.choice = m;
                            self.error = None;
                        }
                    }
                });
                if v.restart_needed {
                    form::inline_warning(ui, &tokens, &t("lan.restart"));
                }
                if let Some(k) = self.error {
                    form::inline_warning(ui, &tokens, &t(k));
                }
                ui.add_space(6.0);
                match self.choice {
                    Mode::Host => self.host_card(ui, &tokens, &v),
                    Mode::Workstation => self.workstation_card(ui, &tokens, &v),
                    Mode::Off => {}
                }
            });
    }

    fn command(&mut self, id: CommandId) -> bool {
        let Some(mut d) = self.link.demo_state() else {
            return false;
        };
        let l = &mut d.lan;
        match id.as_str() {
            "dev.lan_demo_host" => {
                l.chosen = Some(0);
                l.set_host(true);
                self.choice = Mode::Host;
            }
            "dev.lan_demo_workstation" => self.choice = Mode::Workstation,
            "dev.lan_demo_code_burned" => {
                l.code_on = false;
                l.request = None;
                l.code_burned = true;
            }
            "dev.lan_demo_cert_missing" => {
                l.chosen = Some(0);
                l.set_host(true);
                l.cert_missing = true;
                self.choice = Mode::Host;
            }
            "dev.lan_demo_pair_request" => {
                l.code_on = true;
                l.request = Some(crate::company::demo::lan::DemoRequest {
                    label: "Back office".into(),
                    peer: "192.168.1.31".into(),
                });
            }
            _ => return false,
        }
        true
    }

    fn state(&self) -> Value {
        let v = self.control.view();
        json!({
            "screen": "lan_setup",
            "mode": format!("{:?}", v.mode).to_lowercase(),
            "choice": format!("{:?}", self.choice).to_lowercase(),
            "interfaces": v.interfaces.len(),
            "allowed": v.interfaces.iter().filter(|c| c.refusal.is_none()).count(),
            "code_on": matches!(v.code, CodeShown::Active { .. }),
            "code_burned": matches!(v.code, CodeShown::Burned { .. }),
            "request": v.request.is_some(),
            "devices": v.devices.len(),
            "paired": v.paired_host.is_some(),
            "link": v.link,
            "error": self.error,
            "restart_needed": v.restart_needed,
            "host_problem": v.host_problem,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> (tempfile::TempDir, LanSetupScreen) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        (dir, LanSetupScreen::new(link))
    }

    #[test]
    fn host_mode_pairs_a_computer_and_never_puts_the_code_in_state() {
        let (_d, mut s) = screen();
        assert_eq!(s.state()["allowed"], 2);
        assert_eq!(s.pick, Some(0));
        // A refused connection can't be shared on.
        assert_eq!(
            s.control.set_host(true, Some(3)),
            Err("lan.host.iface.none")
        );
        s.control.set_host(true, Some(1)).expect("on");
        assert_eq!(s.state()["mode"], "host");
        assert!(s.command(CommandId::new("dev.lan_demo_pair_request")));
        let st = s.state().to_string();
        assert!(!st.contains(crate::company::demo::lan::CODE));
        assert!(!st.contains(crate::company::demo::lan::FINGERPRINT));
        assert_eq!(s.state()["request"], true);
        s.control.decide(true);
        assert_eq!(s.state()["devices"], 2);
        s.control.remove(0);
        assert_eq!(s.state()["devices"], 1);
        s.control.set_host(false, None).expect("off");
        assert_eq!(s.state()["mode"], "off");
    }

    /// One frame at 1440 x 940 with `events`; the AccessKit nodes.
    fn frame(
        s: &mut LanSetupScreen,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<egui::accesskit::Node> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 940.0),
            )),
            events,
            ..egui::RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
        out.platform_output
            .accesskit_update
            .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
            .unwrap_or_default()
    }

    fn named<'a>(
        nodes: &'a [egui::accesskit::Node],
        role: egui::accesskit::Role,
        name: &str,
    ) -> Option<&'a egui::accesskit::Node> {
        nodes
            .iter()
            .find(|n| n.role() == role && (n.label() == Some(name) || n.value() == Some(name)))
    }

    fn texts(nodes: &[egui::accesskit::Node]) -> String {
        nodes
            .iter()
            .filter_map(|n| n.label().or_else(|| n.value()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Review M2: with the host certificate file missing the window says
    /// so and offers "Make a new certificate"; clicking it removes every
    /// paired computer and clears the problem.
    #[test]
    fn make_a_new_certificate_clears_the_problem_and_every_paired_computer() {
        use egui::accesskit::Role;
        let (_d, mut s) = screen();
        assert!(s.command(CommandId::new("dev.lan_demo_cert_missing")));
        assert_eq!(s.state()["host_problem"], "lan.host.cert_missing");
        assert_eq!(s.state()["devices"], 1);
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        ctx.enable_accesskit();
        let mut nodes = Vec::new();
        for _ in 0..3 {
            nodes = frame(&mut s, &ctx, Vec::new());
        }
        assert!(texts(&nodes).contains(&t("lan.host.cert_missing")));
        // Sharing isn't running: no "Sharing on ..." note, no pairing code.
        assert!(named(&nodes, Role::Button, &t("lan.host.code.new")).is_none());
        assert!(named(&nodes, Role::Button, &t("lan.host.off")).is_some());
        let button = named(&nodes, Role::Button, &t("lan.host.regenerate"))
            .expect("Make a new certificate")
            .clone();
        let b = button.bounds().expect("bounds");
        #[allow(clippy::cast_possible_truncation, reason = "pixel positions")]
        let at = egui::pos2(
            f64::midpoint(b.x0, b.x1) as f32,
            f64::midpoint(b.y0, b.y1) as f32,
        );
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        frame(
            &mut s,
            &ctx,
            vec![egui::Event::PointerMoved(at), press(true)],
        );
        frame(&mut s, &ctx, vec![press(false)]);
        let after = frame(&mut s, &ctx, Vec::new());
        assert_eq!(s.state()["host_problem"], Value::Null);
        assert_eq!(s.state()["devices"], 0);
        assert!(named(&after, Role::Button, &t("lan.host.regenerate")).is_none());
        let audit = s
            .link
            .demo_state()
            .map(|d| d.lan.audit.clone())
            .expect("demo");
        assert_eq!(audit.last(), Some(&"lan_host_key_regenerated"));
        assert!(audit.contains(&"lan_device_removed"));
    }

    /// The burned code says why on the host: how many computers tried and
    /// the last one's address (never in `state()`).
    #[test]
    fn a_burned_code_shows_who_spent_the_tries() {
        let (_d, mut s) = screen();
        assert!(s.command(CommandId::new("dev.lan_demo_host")));
        assert!(s.command(CommandId::new("dev.lan_demo_code_burned")));
        assert_eq!(s.state()["code_burned"], true);
        assert_eq!(s.state()["code_on"], false);
        assert!(
            !s.state()
                .to_string()
                .contains(crate::company::demo::lan::BURN_PEER)
        );
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        ctx.enable_accesskit();
        let mut nodes = Vec::new();
        for _ in 0..3 {
            nodes = frame(&mut s, &ctx, Vec::new());
        }
        let all = texts(&nodes);
        assert!(all.contains(&t("lan.host.code.burned")), "{all}");
        assert!(
            all.contains(&t_count(
                "lan.host.code.burned_from",
                2,
                &[("peer", crate::company::demo::lan::BURN_PEER)]
            )),
            "{all}"
        );
        assert!(all.contains(crate::company::demo::lan::BURN_PEER));
        // A new code clears it.
        s.control.new_code();
        assert_eq!(s.state()["code_burned"], false);
        assert_eq!(s.state()["code_on"], true);
        assert_eq!(
            t_count("lan.host.code.burned_by", 1, &[]),
            "The wrong tries came from one computer."
        );
    }

    #[test]
    fn a_workstation_pairs_with_the_code_from_the_host() {
        let (_d, mut s) = screen();
        assert!(s.command(CommandId::new("dev.lan_demo_workstation")));
        s.control.pair("192.168.1.20:47615", "wrong");
        s.poll();
        assert_eq!(s.state()["error"], "lan.pair.refused.wrong_code");
        s.control
            .pair("192.168.1.20:47615", crate::company::demo::lan::CODE);
        s.poll();
        assert_eq!(s.state()["paired"], true);
        assert_eq!(s.state()["link"], "connected");
        assert_eq!(group_hex("3f9a1c0e7b"), "3f9a 1c0e 7b");
        for k in [
            "lan.purpose",
            "lan.host.firewall",
            "lan.refusal.public",
            "lan.refusal.carrier-nat",
            "lan.pair.refused.wrong_code",
            "lan.link.host-down",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
    }
}
